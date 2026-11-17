<?php

declare(strict_types=1);

// A fatal inside unserialize() does not leave the next unserialize() on the
// worker the table of the call it ended. The engine counts the unserialize()
// calls open on the thread and shares one table among nested ones, running the
// deferred __unserialize() calls and freeing the table only as the outermost
// one ends. A fatal skips that end, so left alone the count stays raised for
// good: every later call is taken for a nested one, its __unserialize() calls
// never run, and its back-references are resolved against a dead table.
//
// Without ?action, while this request is parked: the shutdown function the
// failing request runs after a fatal in its handler, and a request the worker
// starts after a fatal in a shutdown function — sent from an async task so that
// it is queued while the failing request still runs, and admitted by the event
// loop since this request is parked throughout. ?action=trigger fails in a shutdown function of its own
// after its answer has gone out, and ?action=check, the request after it, is
// started with nothing parked on the worker.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/session_inner_request.php';
require_once __DIR__ . '/thread_cell_probe.php';

$action = (string) ($_GET['action'] ?? '');
$t = new TestCase('unserialize_state_after_a_fatal' . ($action !== '' ? '_' . $action : ''), 'fibers');

if ($action === 'trigger') {
    ini_set('display_errors', '0');
    register_shutdown_function(static function (): void {
        oxphp_finish_request();
        set_error_handler(static function (): bool {
            trigger_error('fatal inside unserialize() after the answer', E_USER_ERROR);
        });
        unserialize('garbage');
    });
    $t->assertTrue('the shutdown function that fails is registered', true);
    $t->done();
}

if ($action === 'check') {
    $probe = thread_cell_unserialize_probe();
    $t->meta('probe', $probe);
    $t->assertTrue('a back-reference names the object of this call', $probe['same']);
    $t->assertTrue('and its __unserialize() ran', $probe['filled']);
    $t->done();
}

$sharedState['cell_unser_shutdown'] = null;
$sharedState['cell_unser_next'] = null;
$next = session_arrival_send('/tests/fibers/fixture_unserialize_after_fatal.php?role=probe', 0.1);
$failed = fiber_inner_request('/tests/fibers/fixture_unserialize_after_fatal.php?role=fail', 8.0);
$nextRanFirst = $sharedState['cell_unser_next'] !== null;
$arrival = oxphp_async_await($next, 8.0);

$shutdown = $sharedState['cell_unser_shutdown'];
$probe = $sharedState['cell_unser_next'];
$t->meta('shutdown', $shutdown);
$t->meta('probe', $probe);
$t->meta('arrival', $arrival);
$t->meta('next_ran_before_this_resumed', $nextRanFirst);

$t->assertNotContains('the failing request failed in its handler', $failed, 'not reached');
$t->assertTrue('it ran its shutdown functions', is_array($shutdown));
$t->assertTrue(
    'after a fatal in the handler, a back-reference in a shutdown function names its own object',
    $shutdown['same'] ?? null
);
$t->assertTrue('and its __unserialize() ran', $shutdown['filled'] ?? null);

$t->assertTrue(
    'after a fatal in a shutdown function, a back-reference in the next request names its own object',
    $probe['same'] ?? null
);
$t->assertTrue('and its __unserialize() ran', $probe['filled'] ?? null);

$t->done();
