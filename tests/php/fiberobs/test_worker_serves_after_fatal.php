<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An ordinary request between the two fatals, and another after them.
//
// A request that returns from what it calls leaves the chain as it found it —
// its frames are pushed and popped in balance. So this says nothing about the
// chain; what it says is that the worker survived a fatal and is serving, the
// same ground the traditional-mode bailout suite covers.

$t = new TestCase('worker_serves_after_fatal', 'fiberobs');

$worker = OxPHP\Server\Worker::current();

$t->assertTrue('worker mode is on', OxPHP\Server\Worker::isWorkerMode());
$t->assertGreaterThan('the worker has served a fatal already', $worker->requestCount(), 1);
$t->assertFalse('no exit is scheduled yet', $worker->isExitScheduled());

// User calls of its own, so this request pushes and pops frames on the chain
// rather than only running the entry closure.
$sum = (static function (): int {
    $step = static fn (int $n): int => $n + 1;

    return $step($step($step(0)));
})();
$t->assertSame('user functions still run', $sum, 3);

$t->done();
