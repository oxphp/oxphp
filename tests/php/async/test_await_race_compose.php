<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_race_compose', 'async');

// Run with ASYNC_WORKERS=1. A task that races nested promises must suspend
// cooperatively so the single worker is free to run them. A blocking race
// would pin the only worker and deadlock.
$outer = oxphp_async(function (): int {
    $fast = oxphp_async(fn (): int => 1);
    $slow = oxphp_async(function (): int {
        usleep(500000);
        return 2;
    });
    $r = oxphp_async_await_race([$fast, $slow]);
    return $r['value']; // first to settle wins
});

$result = oxphp_async_await($outer, 3.0);

$t->assertSame(
    'await_race composes inside a fiber on a single worker',
    $result,
    1
);

$t->done();
