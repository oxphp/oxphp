<?php

declare(strict_types=1);

// array_multisort() refuses a fiber switch in traditional mode too, where the
// request does not run on a fiber of the server's: Fiber::suspend() in one of
// its comparisons throws FiberError, and the sort goes on to finish. The sort
// keeps its table of comparison functions on the thread for the whole call and
// puts back what it found there as it returns. Two Fibers that each suspended
// in a sort and were resumed in the order they started would leave the second
// sort calling through what the first put back — nothing — and crash the
// process, with every request it was serving.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../fibers/multisort_probe.php';

$refusals = [];
$sortInFiber = static function (string $who) use (&$refusals): Fiber {
    return new Fiber(static function () use ($who, &$refusals): array {
        return multisort_probe_sort(static function () use ($who, &$refusals): void {
            try {
                Fiber::suspend($who);
                $refusals[$who] = 'suspended';
            } catch (\FiberError $e) {
                $refusals[$who] = 'FiberError: ' . $e->getMessage();
            }
        });
    });
};

$first = $sortInFiber('first');
$second = $sortInFiber('second');
$first->start();
$second->start();
foreach ([$first, $second] as $fiber) {
    if ($fiber->isSuspended()) {
        $fiber->resume();
    }
}

$after = new Fiber(static fn () => Fiber::suspend('suspended after the sorts'));
try {
    $afterSuspended = $after->start();
} catch (\FiberError $e) {
    $afterSuspended = 'FiberError: ' . $e->getMessage();
}
if ($after->isSuspended()) {
    $after->resume();
}

$t = new TestCase('array_multisort_refuses_a_fiber_switch_outside_a_request_fiber', 'hooks');
$blocked = 'FiberError: Cannot switch fibers in current execution context';
$sorted = ['strings' => ['a', 'b', 'c'], 'order' => [1, 2, 3]];
$t->assertSame('Fiber::suspend() inside the first sort throws FiberError', $refusals['first'] ?? null, $blocked);
$t->assertSame('Fiber::suspend() inside the second sort throws FiberError', $refusals['second'] ?? null, $blocked);
$t->assertSame('the first sort finishes', $first->isTerminated() ? $first->getReturn() : null, $sorted);
$t->assertSame('the second sort finishes', $second->isTerminated() ? $second->getReturn() : null, $sorted);
$t->assertSame('a fiber suspends once the sorts have returned', $afterSuspended, 'suspended after the sorts');

$t->done();
