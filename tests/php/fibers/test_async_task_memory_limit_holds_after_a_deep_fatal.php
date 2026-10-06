<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// fibers/test_memory_limit_holds_after_a_deep_fatal for an oxphp_async() task.
// A task that runs out of memory deep in a call stack ends there, but the thread
// it ran on goes on running other tasks, and those have to find memory_limit
// enforced: on PHP 8.5 the fatal's backtrace is taken past the limit, and a
// thread whose allocator went on counting that memory as in use would enforce no
// limit for the rest of its life. This profile runs one async worker, so every
// task here runs on the same thread.
//
// Neither displayed nor logged, for the reason the fixture gives. A task's ini
// changes stay on its thread, so the task after it puts both back.

$t = new TestCase('async_task_memory_limit_holds_after_a_deep_fatal', 'fibers');

$fatal = oxphp_async(static function (): void {
    set_error_handler(null);
    ini_set('display_errors', '0');
    ini_set('log_errors', '0');
    $descend = static function (callable $self, int $depth): void {
        if ($depth > 0) {
            $self($self, $depth - 1);
            return;
        }
        $s = 'x';
        for (;;) {
            $s .= $s;
        }
    };
    $descend($descend, 400_000);
});

$fatalMessage = null;
try {
    oxphp_async_await($fatal, 20.0);
} catch (\Throwable $e) {
    $fatalMessage = $e->getMessage();
}
$t->meta('fatal task', $fatalMessage);

$heap = oxphp_async_await(oxphp_async(static function (): array {
    ini_restore('display_errors');
    ini_restore('log_errors');

    return [memory_get_usage(true), ini_parse_quantity((string) ini_get('memory_limit'))];
}), 5.0);
$t->meta('heap after the fatal task', $heap);

$enforce = oxphp_async(static function (): string {
    set_error_handler(null);
    $big = str_repeat('x', 200 << 20);

    return 'NOT-ENFORCED: ' . strlen($big);
});

$enforced = null;
$given = null;
try {
    $given = oxphp_async_await($enforce, 5.0);
} catch (\Throwable $e) {
    $enforced = $e->getMessage();
}
$t->meta('task asking for more than the limit', $given ?? $enforced);

$t->assertTrue('the deep task ended in a fatal', $fatalMessage !== null);

[$real, $limit] = $heap;
$t->assertTrue('the allocator counts less than memory_limit as in use after it', $real <= $limit);
$t->assertTrue('a later task is refused more than the limit', $enforced !== null);
$t->assertNull('rather than given it', $given);

$t->done();
