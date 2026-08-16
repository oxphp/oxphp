<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_all_compose', 'async');

// Run with ASYNC_WORKERS=1. A task that awaits a fan-out of nested promises
// must suspend cooperatively so the single worker is free to run those nested
// promises. A blocking await_all would pin the only worker and deadlock.
$outer = oxphp_async(function (): array {
    $a = oxphp_async(fn (): int => 1);
    $b = oxphp_async(fn (): int => 2);
    $r = oxphp_async_await_all([$a, $b]);
    return [$r[$a], $r[$b]]; // normalise to positional order
});

$result = oxphp_async_await($outer, 3.0);

$t->assertSame(
    'await_all composes inside a fiber on a single worker',
    $result,
    [1, 2]
);

$t->done();
