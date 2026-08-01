<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('fiber_identity', 'fibers');

// Characterization, not a wish: this pins what the engine reports about an
// OxPHP request fiber today. Each assertion below is expected to invert when
// request fibers become real Fiber objects, and inverting them is a deliberate
// edit, not a surprise.
$t->assertTrue('running in worker mode', oxphp_is_worker());

// A request fiber is a raw fiber context with no Fiber object behind it, so the
// engine reports the request as {main}.
$t->assertNull('Fiber::getCurrent() inside a request', \Fiber::getCurrent());

// Consequence of the same fact: userland cannot suspend the request.
$t->assertThrows(
    'Fiber::suspend() from a request throws',
    static fn () => \Fiber::suspend(),
    \FiberError::class
);

$t->done();
