<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';

$t = new TestCase('foreign_resume_is_refused', 'fibers');

// A request runs as a real \Fiber, so anything it calls can take hold of that
// object and resume it later. Nothing in the server is waiting on such a wake
// and none of the request's state is installed for it, so it must be refused
// rather than obeyed.
$self = \Fiber::getCurrent();
$t->assertNotNull('the request has a fiber to hand out', $self);
ParkedRequestFiber::set($self);

// Parks this fiber on the socket read and lets the worker pick the inner
// request up. PHP_WORKERS=1 makes that the same thread, so the inner request
// runs while this one is parked and can reach the fiber left above.
$body = fiber_inner_request('/tests/fibers/fixture_resume_parked.php');
$inner = json_decode($body, true);

$t->assertTrue('inner request was served on the same worker', is_array($inner));
$t->assertContains('inner request completed', $body, 'inner-done');
$t->assertTrue('the inner request found this request parked', $inner['was_suspended'] ?? false);

$t->assertContains(
    'resuming a parked request fiber is refused',
    (string) ($inner['resume'] ?? ''),
    'FiberError: Cannot resume an OxPHP request fiber from userland'
);
$t->assertContains(
    'throwing into a parked request fiber is refused',
    (string) ($inner['throw'] ?? ''),
    'FiberError: Cannot resume an OxPHP request fiber from userland'
);

// The refusals left this request parked where it was rather than unwinding it —
// which is why the read above completed and this line runs at all.
$t->assertTrue('the request stayed parked through both attempts', $inner['still_suspended'] ?? false);
$t->assertSame('the request is still running as the same fiber', \Fiber::getCurrent(), $self);

// And still usable: it can park and be resumed by the scheduler again.
$before = microtime(true);
oxphp_sleep(0.02);
$t->assertGreaterThan('the request can still park and be resumed', microtime(true) - $before, 0.015);

$t->done();
