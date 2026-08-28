<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// After the retire. Whichever worker answers this, it has to be one the pool
// can still reach: the request before it asked its worker to leave, and a
// worker that walks a chain of open calls with no end never finishes leaving.
//
// Nothing is asserted about which worker it is. The replacement usually answers
// — it counts this as its first request — but the retiring one is allowed to
// serve until it goes, and pinning that down would only make the test flake.

$t = new TestCase('pool_serves_after_retire', 'fiberobs');

$t->assertTrue('worker mode is on', OxPHP\Server\Worker::isWorkerMode());
$t->assertGreaterThan('a worker is serving', OxPHP\Server\Worker::current()->requestCount(), 0);

$sum = (static function (): int {
    $step = static fn (int $n): int => $n + 1;

    return $step($step($step(0)));
})();
$t->assertSame('user functions still run', $sum, 3);

$t->done();
