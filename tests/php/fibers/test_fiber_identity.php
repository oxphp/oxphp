<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('fiber_identity', 'fibers');

// Characterization: this pins what the engine reports about an OxPHP request
// fiber. Changing any of it is a deliberate edit, not a surprise.
$t->assertTrue('running in worker mode', oxphp_is_worker());

// A request runs as a real Fiber object, so the engine reports the request as
// itself rather than as {main}. This is what lets an event loop, a context
// storage or a fiber-local tell two concurrent requests apart.
$current = \Fiber::getCurrent();
$t->assertNotNull('Fiber::getCurrent() inside a request', $current);
$t->assertInstanceOf('the current fiber is a Fiber', $current, \Fiber::class);
$t->assertTrue('the current fiber reports itself running', $current->isRunning());

$t->done();
