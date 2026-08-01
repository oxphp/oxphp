<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('nested_userland_fiber', 'fibers');

// A userland fiber started inside a request must keep working, and OxPHP's
// suspend points must refuse to switch while one is running — the scheduler
// cannot resume a context it does not own, so oxphp_sleep() takes its blocking
// path there instead. Both halves must survive the migration unchanged.
$fiber = new \Fiber(static function (): string {
    $inside = \Fiber::getCurrent();
    $value = \Fiber::suspend('parked');

    return $value . ':' . ($inside !== null ? 'has-current' : 'no-current');
});

$parked = $fiber->start();
$t->assertEqual('userland fiber suspends with its value', $parked, 'parked');
$t->assertTrue('userland fiber is suspended', $fiber->isSuspended());

$fiber->resume('resumed');
$t->assertEqual(
    'userland fiber sees itself as current',
    $fiber->getReturn(),
    'resumed:has-current'
);

// oxphp_sleep() inside a userland fiber falls back to a blocking sleep rather
// than suspending the outer request fiber. Measured, not asserted structurally:
// the call must take at least its nominal duration and must not throw.
$blocking = new \Fiber(static function (): float {
    $t0 = microtime(true);
    oxphp_sleep(0.05);

    return microtime(true) - $t0;
});
$blocking->start();
$t->assertGreaterThan('oxphp_sleep in a userland fiber still sleeps', $blocking->getReturn(), 0.04);

$t->done();
