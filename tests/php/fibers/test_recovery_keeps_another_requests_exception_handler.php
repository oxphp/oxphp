<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recovery_probe.php';

// An exception handler one request installs is not called for a throw raised
// while the worker recovers from another request's fatal.
//
// The handler is kept on the thread, and the thread carries every request a
// worker multiplexes. This request installs one and parks; the inner request
// fatals, and the worker's recovery from that fatal runs a filter's onClose()
// that throws, with no frame of the inner request to unwind into. Handing that
// throw to the handler installed on the thread would run this request's code
// inside the inner request, writing into the inner request's response.

$t = new TestCase('recovery_keeps_another_requests_exception_handler', 'fibers');

OxphpRecoveryProbe::reset();
$handler = static function (\Throwable $e): void {
    OxphpRecoveryProbe::$foreignHandlerRan = true;
    echo "ANOTHER-REQUESTS-EXCEPTION-HANDLER\n";
};
set_exception_handler($handler);

$sock = recovery_inner_send('/tests/fibers/fixture_recovery_throw_in_filter_close.php');
$t->assertTrue('inner request socket connected', $sock !== false);
$resp = $sock === false ? '' : recovery_inner_read($sock);

// set_exception_handler() answers with the handler it replaces, so this reads
// whether the slot still holds this request's handler, and leaves it there for
// the two restores below to take off.
$still = set_exception_handler($handler);
restore_exception_handler();
restore_exception_handler();

$t->meta('response', $resp);
$t->assertContains('the inner request fataled', $resp, ' 500 ');
// Without it the next two prove nothing: no throw, nothing to hand over.
$t->assertTrue("the filter's onClose() ran as the worker gave the stream back", OxphpRecoveryProbe::$closeRan);
$t->assertFalse("this request's exception handler was not called for it", OxphpRecoveryProbe::$foreignHandlerRan);
$t->assertNotContains("and wrote nothing into the inner request's response", $resp, 'ANOTHER-REQUESTS-EXCEPTION-HANDLER');
$t->assertTrue('the handler is still installed for this request', $still === $handler);

$t->done();
