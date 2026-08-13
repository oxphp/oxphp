<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// filter_*(INPUT_ENV) must go on answering for the whole life of a worker.
//
// ext/filter does not read $_ENV. For INPUT_ENV it reads the engine's own slot,
// PG(http_globals)[TRACK_VARS_ENV], and reports "no such variable" for anything
// that is not an array there. In worker mode $_ENV is process state — its
// callback would rebuild the array from the process environment and discard what
// a .env loader wrote at a boot that never runs again — so the callback is
// disarmed once the array exists, and nothing rebuilds that slot afterwards.
// The slot therefore has to survive the request boundary on its own.
//
// Three requests, three places it can be lost, all on one worker (PHP_WORKERS=1
// in this profile):
//
//   warm    — materialise $_ENV, so the disarm is engaged from the next reset on.
//             Until the array exists the callback is still armed and rebuilds the
//             slot on demand, which would make the checks below pass on any build.
//             It also checks that $_ENV carries the process environment at all,
//             and seeds the key the negative assertion below is about.
//   expect  — a later request reads the slot the per-request reset left standing.
//   suspend — the slot must not travel with a suspended request: the inner
//             self-request runs inside this one's parked window and reads it
//             there, and this request reads it again after resuming.
//
// PATH rather than a name of our own: the slot is a snapshot of the process
// environment taken when $_ENV was first built, so getenv() is the independent
// reader to compare it against. Values the application writes into $_ENV are a
// different array by then (a userland write separates it by COW) and are not
// visible through INPUT_ENV — in stock PHP either.

$t = new TestCase('env_filter_input', 'hooks');

$mode = $_GET['mode'] ?? '';
$t->meta('mode', $mode);

/** Read the environment through every INPUT_ENV entry point ext/filter has. */
$readFilterEnv = static function (): array {
    return [
        'has' => filter_has_var(INPUT_ENV, 'PATH'),
        'one' => filter_input(INPUT_ENV, 'PATH'),
        'all' => filter_input_array(INPUT_ENV),
    ];
};

$assertFilterEnv = static function (string $when) use ($t, $readFilterEnv): void {
    $path = getenv('PATH');
    $seen = $readFilterEnv();

    $t->assertTrue("getenv(PATH) is a usable string $when", is_string($path) && $path !== '');
    $t->assertSame("filter_has_var(INPUT_ENV, PATH) $when", $seen['has'], true);
    $t->assertSame("filter_input(INPUT_ENV, PATH) $when", $seen['one'], $path);
    $t->assertSame(
        "filter_input_array(INPUT_ENV) carries PATH $when",
        is_array($seen['all']) ? ($seen['all']['PATH'] ?? null) : $seen['all'],
        $path
    );

    // The other half of the published contract, and the half nothing else pins:
    // what these functions read is the process environment, not $_ENV, so a value
    // the application wrote into $_ENV is invisible through them. ?mode=warm put
    // this key there — a userland write separates the array from the engine's
    // copy by COW, and the engine's copy is what ext/filter reads. True in any
    // SAPI; asserted here because the docs say so and worker mode is where the
    // two arrays live long enough for anyone to notice.
    // The positive half first, and it is load-bearing: without it the negative
    // assertion below passes on a build where ?mode=warm's write never reached
    // this request at all, which is not what is being claimed.
    $t->assertSame(
        "the \$_ENV write from an earlier request is still there $when",
        $_ENV['OX_FILTER_ENV_PROBE'] ?? null,
        'written-by-the-application'
    );
    $t->assertNull(
        "filter_input(INPUT_ENV, …) does not report a value written into \$_ENV $when",
        filter_input(INPUT_ENV, 'OX_FILTER_ENV_PROBE')
    );
};

if ($mode === 'warm') {
    // eval() rather than a plain mention: it compiles on every request whatever
    // OPcache holds, so the auto-global is materialised here and not on whichever
    // earlier request happened to compile a file naming $_ENV. A fixed literal,
    // and the compile step is the lever being pulled — nothing here is derived
    // from input.
    eval('$ignored = $_ENV;');

    // The precondition the two lines after this one rest on: $_ENV really was
    // built from the process environment, which happens only with 'E' in
    // variables_order. Checked against a key the environment has, not against
    // "the array is not empty" — an earlier test in this profile seeds a key of
    // its own into $_ENV on this same worker, so a non-empty array is true
    // whatever variables_order says. Without 'E' the INPUT_ENV reads below would
    // fail and be read as a server regression, which is the substitution this
    // line exists to prevent.
    $path = getenv('PATH');
    $t->assertTrue('getenv(PATH) is a usable string', is_string($path) && $path !== '');
    $t->assertSame('$_ENV was built from the process environment', $_ENV['PATH'] ?? null, $path);

    // And a value written into $_ENV the way an application writes one, for the
    // negative assertion the two reading lines make about it. Seeded here rather
    // than borrowed from the neighbouring test's key so this block does not
    // depend on what else the profile ran; it survives to those requests because
    // worker mode pins $_ENV, which is a different test's guarantee and this
    // one's precondition.
    $_ENV['OX_FILTER_ENV_PROBE'] = 'written-by-the-application';

    $t->done();
}

if ($mode === 'suspend') {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('inner self-request socket connected', $sock !== false);
    stream_set_timeout($sock, 5);
    fwrite($sock, "GET /tests/hooks/fixture_inner_env_filter.php?tag=inner HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    sleep(2);                               // hooked: suspends this request fiber

    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    // Without this the rest proves nothing: if the inner request never ran inside
    // the parked window, the slot was never at risk of travelling anywhere.
    $t->assertContains('inner request was served while this one was parked', $resp, 'INNER-OK');

    $assertFilterEnv('after resuming from a hooked sleep');
    $t->done();
}

$assertFilterEnv('on a later request of the same worker');
$t->done();
