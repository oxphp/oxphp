<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// What a request sets with putenv() must end with that request, as it does when
// PHP shuts the request down — and what the worker's bootstrap set must not.
//
// putenv() writes the process environment and keeps the value it replaced in a
// table of its own, per thread; only the destructor of that table puts the old
// value back, and the engine runs it at request shutdown. A worker runs request
// shutdown once in its life, so without a rollback of its own one request's
// putenv() is read through getenv() and inherited by the child processes of
// every request the worker serves afterwards.
//
// All on one worker (PHP_WORKERS=1 in this profile):
//
//   seed   — reads the bootstrap's OX_PUTENV_BOOT (the control: what the worker
//            entry script set before oxphp_worker() reaches every request), then
//            sets a new variable and overwrites the bootstrap's. Asserts both are
//            visible to itself, through getenv() and to a child process — the
//            precondition that makes the absences below mean "rolled back".
//   expect — the next requests: the new variable is gone, and the overwritten
//            one is back to the bootstrap's value rather than to what the
//            process started with (it started without it).
//   park   — a request that is still in flight keeps its variable while a
//            request the worker runs alongside it ends and loses its own.
//   after_park — and once the parked request has ended too, its variable is gone.
//
// A child process is asked as well as getenv() because it is the other reader
// the leak reached: it inherits environ as it stands, through no PHP code.

$t = new TestCase('putenv_rolled_back', 'hooks');

$mode = $_GET['mode'] ?? '';
$t->meta('mode', $mode);

/** What a child process started now inherits for $name, or null if unset. */
$childSees = static function (string $name): ?string {
    $out = [];
    $rc = -1;
    exec('printenv ' . escapeshellarg($name), $out, $rc);
    return $rc === 0 ? implode("\n", $out) : null;
};

$assertSet = static function (string $when, string $name, string $expected) use ($t, $childSees): void {
    $t->assertSame("getenv($name) $when", getenv($name), $expected);
    $t->assertSame("a child process inherits $name $when", $childSees($name), $expected);
};

$assertUnset = static function (string $when, string $name) use ($t, $childSees): void {
    $t->assertSame("getenv($name) $when", getenv($name), false);
    $t->assertNull("a child process inherits no $name $when", $childSees($name));
};

if ($mode === 'seed') {
    $assertSet('as the bootstrap set it', 'OX_PUTENV_BOOT', 'boot-value');

    $t->assertTrue('putenv of a new variable succeeds', putenv('OX_PUTENV_REQ=seed-value'));
    $t->assertTrue('putenv over the bootstrap variable succeeds', putenv('OX_PUTENV_BOOT=overwritten'));

    $assertSet('for the request that set it', 'OX_PUTENV_REQ', 'seed-value');
    $assertSet('for the request that overwrote it', 'OX_PUTENV_BOOT', 'overwritten');

    $t->done();
}

if ($mode === 'park') {
    $t->assertTrue('putenv of the outer variable succeeds', putenv('OX_PUTENV_OUTER=outer-value'));

    // The inner request can only be served while this fiber is parked: one
    // worker, and the request is sent just before the hooked sleep below, which
    // lets go of the worker for long enough to serve it. It sets a variable of
    // its own and ends inside this window.
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('inner self-request socket connected', $sock !== false);
    stream_set_timeout($sock, 5);
    fwrite($sock, "GET /tests/hooks/fixture_inner_putenv.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    sleep(2);                               // hooked: suspends this request fiber

    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    // Without this the rest proves nothing. The inner request reports this
    // request's variable as it found it: the environment is the process's, so
    // a request the worker runs alongside this one reads it — and once a
    // request's variables end with it, finding it there also places the inner
    // request inside this one's window rather than after it.
    $t->assertContains('inner request ran while this one was parked', $resp, 'INNER-OK');
    $t->assertContains('inner request read the parked request\'s variable', $resp, '"outer":"outer-value"');

    // The inner request has ended, and what it set ended with it, although
    // this request — the worker's other one in flight — has not.
    $assertUnset('after the inner request that set it has ended', 'OX_PUTENV_INNER');

    // And this request's own variable is still its own: the end of the inner
    // request is not the end of this one.
    $assertSet('for the parked request, after another request ended', 'OX_PUTENV_OUTER', 'outer-value');

    $t->done();
}

if ($mode === 'after_park') {
    $assertUnset('after the parked request that set it has ended', 'OX_PUTENV_OUTER');
    $assertUnset('after the inner request that set it has ended', 'OX_PUTENV_INNER');

    $t->done();
}

// ?mode=expect — a request after the seed, with nothing set of its own.
$assertUnset('after the request that set it has ended', 'OX_PUTENV_REQ');
$assertSet('after the request that overwrote it has ended', 'OX_PUTENV_BOOT', 'boot-value');

$t->done();
