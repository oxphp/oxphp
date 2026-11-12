<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';
require_once __DIR__ . '/recovery_probe.php';

// Application code the worker runs while it releases a session, after the save
// handler's write() fataled inside a Fiber of its own, runs in the request's
// fiber.
//
// A fatal ending a fiber clears the engine's record of the current fiber on
// its way into the request's, and the steps of the release that follow run
// application code: here an error handler, handed the warning the session
// module raises when it refuses the close. Fiber::getCurrent() there has to
// answer with the request's fiber, as it does everywhere else in the request.

$t = new TestCase('session_release_after_a_fiber_fatal_keeps_the_request_fiber', 'fibers');

OxphpRecoveryProbe::reset();
$body = session_inner_request('/tests/fibers/fixture_recovery_session_write_fatal.php', str_repeat('8d', 16));
// The inner request left its error handler installed; this takes it off.
restore_error_handler();

$t->meta('body', $body);
$t->meta('fibers', ['request' => OxphpRecoveryProbe::$requestFiber, 'at_warning' => OxphpRecoveryProbe::$fiberAtWarning]);

$t->assertNotNull('the inner request ran in a fiber', OxphpRecoveryProbe::$requestFiber);
// Without it the last assertion proves nothing: no warning, no code run in
// the release.
$t->assertTrue(
    'the release went on to the close and the session module refused it',
    OxphpRecoveryProbe::$warned
);
$t->assertSame(
    'the error handler that warning reached ran in the request fiber',
    OxphpRecoveryProbe::$fiberAtWarning,
    OxphpRecoveryProbe::$requestFiber
);

$t->done();
