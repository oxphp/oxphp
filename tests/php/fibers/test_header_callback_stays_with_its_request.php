<?php

declare(strict_types=1);

// A header callback goes with the request that registered it when that request
// parks. The engine keeps the callback per thread until the headers are sent,
// so one left on the thread by a parked request would run in the headers of
// the next request sent on the worker, and the request that registered it
// would find it gone when its own headers go out.
//
// A holder registers a callback and parks. A neighbour sent while it is parked
// registers none.
//
// One a request leaves behind is not run for the request after it either. A
// leaver leaves one standing, and the request sent after it starts while this
// one is parked waiting for it. The one left standing is still let go of.
// Again with the next request queued while a busy leaver runs, so that the
// worker starts it as the leaver ends, before this one resumes.
//
// ?action=trigger leaves one the same way, and ?action=check, the request
// after it, starts with nothing parked on the worker; its suite line asserts
// that its headers lack the callback's.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';
require_once __DIR__ . '/session_inner_request.php';

$action = (string) ($_GET['action'] ?? '');
if ($action === 'trigger') {
    header_register_callback(static function (): void {
        header_register_callback(static function (): void {
            header('X-Header-Callback: left-standing');
        });
    });
    $t = new TestCase('header_callback_stays_with_its_request_trigger', 'fibers');
    $t->assertTrue('a callback is registered to leave another standing', true);
    $t->done();
}
if ($action === 'check') {
    echo 'check';

    return;
}

$sharedState['cell_cb_parked'] = false;
$holderSock = thread_cell_send('/tests/fibers/fixture_header_callback.php?who=holder&cb=1&park=600');
$parked = thread_cell_wait_for($sharedState, 'cell_cb_parked');
$neighbour = thread_cell_read(thread_cell_send('/tests/fibers/fixture_header_callback.php?who=neighbour'));
$holder = thread_cell_read($holderSock);
$releasedBefore = class_exists('ThreadCellCallbackProbe', false) ? ThreadCellCallbackProbe::$released : 0;
$leaver = thread_cell_read(thread_cell_send('/tests/fibers/fixture_header_callback.php?who=leaver&cb=nested'));
$after = thread_cell_read(thread_cell_send('/tests/fibers/fixture_header_callback.php?who=after'));
$released = ThreadCellCallbackProbe::$released - $releasedBefore;

$sharedState['cell_cb_reader_resumed'] = false;
$sharedState['cell_cb_busy_done'] = false;
$queuedPromise = session_arrival_send('/tests/fibers/fixture_header_callback.php?who=queued', 0.15);
$busyLeaver = thread_cell_read(thread_cell_send('/tests/fibers/fixture_header_callback.php?who=busy-leaver&cb=nested&busy=400'));
$sharedState['cell_cb_reader_resumed'] = true;
$queued = oxphp_async_await($queuedPromise, 8.0);
$queuedReleased = ThreadCellCallbackProbe::$released - $releasedBefore - $released;

$t = new TestCase('header_callback_stays_with_its_request', 'fibers');
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);
$t->meta('leaver', $leaver);
$t->meta('after', $after);
$t->meta('leaver_callback_released', $released);
$t->meta('busy_leaver', $busyLeaver);
$t->meta('queued', $queued);

$t->assertTrue('the holder parked', $parked);
$t->assertSame(
    'the neighbour ran while the holder was parked',
    json_decode($neighbour['body'], true)['holder_parked'] ?? null,
    true
);
$t->assertNotContains("the holder's callback did not run in the neighbour's headers", strtolower($neighbour['headers']), 'x-header-callback');
$t->assertContains("the holder's callback ran in its own headers", strtolower($holder['headers']), 'x-header-callback: holder');
$t->assertNotContains("the leaver's callback did not run in the next request's headers", strtolower($after['headers']), 'x-header-callback');
$t->assertSame("and was let go of", $released, 1);
$t->assertSame(
    'the request queued behind the busy leaver started after it was done',
    json_decode($queued['body'] ?? '', true)['busy_done'] ?? null,
    true
);
$t->assertSame(
    'and before this one resumed',
    json_decode($queued['body'] ?? '', true)['reader_resumed'] ?? null,
    false
);
$t->assertNotContains("the busy leaver's callback did not run in its headers", strtolower($queued['headers'] ?? ''), 'x-header-callback');
$t->assertSame('and was let go of', $queuedReleased, 1);

$t->done();
