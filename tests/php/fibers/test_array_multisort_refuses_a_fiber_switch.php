<?php

declare(strict_types=1);

// Inside array_multisort() in a worker-mode request or an oxphp_async() task, a
// fiber switch throws FiberError, and once the sort returns, switching works
// again: the sort runs with switching blocked, so that nothing parks while its
// table of comparison functions is on the thread.
//
// The sort runs inside a Fiber this request starts, and its first comparison
// calls Fiber::suspend().

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/multisort_probe.php';

$fiber = new Fiber(static fn (): array => multisort_probe_sort(static function (): void {
    Fiber::suspend('suspended inside the sort');
}));

$refused = null;
$suspended = null;
try {
    $suspended = $fiber->start();
} catch (\FiberError $e) {
    $refused = $e->getMessage();
}
if ($fiber->isSuspended()) {
    $fiber->resume();
}

$after = new Fiber(static fn () => Fiber::suspend('suspended after the sort'));
try {
    $afterSuspended = $after->start();
} catch (\FiberError $e) {
    $afterSuspended = 'FiberError: ' . $e->getMessage();
}
if ($after->isSuspended()) {
    $after->resume();
}

// The same in an oxphp_async() task, which runs in a fiber of the server's as
// well. A task cannot declare a class, so the comparison reaches userland
// through the error handler of the warning a string sort of arrays raises.
$task = oxphp_async(static function (): string {
    $fiber = new Fiber(static function (): void {
        set_error_handler(static function (): bool {
            Fiber::suspend('suspended inside the sort');

            return true;
        });
        try {
            $rows = [[2], [1]];
            array_multisort($rows, SORT_STRING);
        } finally {
            restore_error_handler();
        }
    });
    try {
        $outcome = 'suspended: ' . $fiber->start();
    } catch (\FiberError $e) {
        $outcome = 'FiberError: ' . $e->getMessage();
    }
    if ($fiber->isSuspended()) {
        $fiber->resume();
    }

    return $outcome;
});
$inTask = oxphp_async_await($task, 5.0);

$t = new TestCase('array_multisort_refuses_a_fiber_switch', 'fibers');
$t->meta('suspended', $suspended);

$t->assertSame('Fiber::suspend() inside the sort throws FiberError', $refused, 'Cannot switch fibers in current execution context');
$t->assertTrue('the fiber ran to its end', $fiber->isTerminated());
$t->assertSame('a fiber suspends once the sort has returned', $afterSuspended, 'suspended after the sort');
$t->assertSame('in a task, Fiber::suspend() inside the sort throws FiberError', $inTask, 'FiberError: Cannot switch fibers in current execution context');

$t->done();
