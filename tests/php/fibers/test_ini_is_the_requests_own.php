<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/ini_probe_state.php';

// An ini directive a request changes belongs to that request, including while
// the worker is running another one beside it.
//
// The engine keeps ini values per thread, and a worker runs every request it
// multiplexes on one thread. So unless the values travel with the request, a
// request that parks hands what it set to whatever the worker runs in the
// window, and comes back to whatever that one set. ignore_user_abort() is the
// directive where this costs most: read by a neighbour, it decides whether that
// neighbour is stopped when its own client leaves.
//
// This request changes two directives and parks on a read of an inner request,
// which changes two of its own — one of the same and one this request never
// touched — and finishes inside the window. Each half is read from where it
// lands:
//
//   - the inner request must start from the values the worker booted with;
//   - this one must come back to exactly its own values, which also means the
//     directive only the inner request changed is back at the boot value.
//
// The inner request also tightens open_basedir, which is not one of the
// directives that travel: it stays on the worker while this request is still
// in flight, and fibers/test_ini_the_worker_keeps_goes_back_with_the_last_request
// reads it once this request, the last one out, has ended. This request leaves
// a fire-and-forget task behind for that one: a worker still reclaiming a
// task's promise is not idle, so the next request is taken without the reset
// an idle worker runs first, and what it finds is what this request's own end
// left.

if (!isset($iniBoot) || !is_array($iniBoot)) {
    http_response_code(500);
    echo "FAIL: \$iniBoot is not in scope — this test needs the worker entry file"
        . " at tests/fixtures/worker/worker_entry.php\n";
    return;
}

$t = new TestCase('ini_is_the_requests_own', 'fibers');

// Not in the boot capture; what this request finds on entry is the baseline for
// it. A request reaching here the ordinary way has just been through the reset
// between requests, which is what makes that reading one.
$iuaEntry = ini_get('ignore_user_abort');
// Likewise, and this request leaves it alone.
$basedirEntry = ini_get('open_basedir');
OxphpIniProbeState::$basedirBaseline = $basedirEntry;

$mine = [
    'precision'         => $iniBoot['precision'] === '3' ? '4' : '3',
    'ignore_user_abort' => $iuaEntry === '1' ? '0' : '1',
];

ini_set('precision', $mine['precision']);
ignore_user_abort($mine['ignore_user_abort'] === '1');

$t->assertSame('precision took before the park', ini_get('precision'), $mine['precision']);
$t->assertSame('ignore_user_abort took before the park', ini_get('ignore_user_abort'), $mine['ignore_user_abort']);

$body = fiber_inner_request('/tests/fibers/fixture_ini_probe.php');
$inner = json_decode($body, true);

$after = [
    'precision'         => ini_get('precision'),
    'user_agent'        => ini_get('user_agent'),
    'ignore_user_abort' => ini_get('ignore_user_abort'),
    'open_basedir'      => ini_get('open_basedir'),
];

// Put back before any assertion can end the request, so a failure here is not
// also a failure of every test after it on this worker.
ini_restore('precision');
ini_restore('ignore_user_abort');

$t->assertTrue('the inner request answered with JSON', is_array($inner));
if (!is_array($inner)) {
    $t->meta('body', $body);
    $t->done();
}

$t->assertSame(
    'the inner request changed its own values',
    $inner['set'] ?? null,
    ['precision' => '11', 'user_agent' => 'oxphp-inner-ini-probe', 'open_basedir' => '/var/www/html:/tmp']
);

$t->assertSame(
    'the request served in the window starts with the boot precision, not the parked request\'s',
    $inner['seen']['precision'] ?? null,
    $iniBoot['precision']
);
$t->assertSame(
    'and with ignore_user_abort as a request starts with it, not as the parked request set it',
    $inner['seen']['ignore_user_abort'] ?? null,
    $iuaEntry
);
$t->assertSame(
    'and with the boot user_agent',
    $inner['seen']['user_agent'] ?? null,
    $iniBoot['user_agent']
);

$t->assertSame(
    'this request resumes with the precision it set, not the neighbour\'s',
    $after['precision'],
    $mine['precision']
);
$t->assertSame(
    'and with the ignore_user_abort it set',
    $after['ignore_user_abort'],
    $mine['ignore_user_abort']
);
$t->assertSame(
    'and a directive only the neighbour changed is back at the boot value',
    $after['user_agent'],
    $iniBoot['user_agent']
);
$t->assertSame(
    'and a directive that does not travel is still the neighbour\'s while this request is in flight',
    $after['open_basedir'],
    '/var/www/html:/tmp'
);

// The task the next test relies on; see the header. is_file before unlink, not
// the silence operator: a registered error handler still runs under @.
if (is_file(OxphpIniProbeState::TASK_DONE)) {
    unlink(OxphpIniProbeState::TASK_DONE);
}
oxphp_async(static function (): void {
    time_nanosleep(2, 0);
    file_put_contents(OxphpIniProbeState::TASK_DONE, 'done');
});

$t->done();
