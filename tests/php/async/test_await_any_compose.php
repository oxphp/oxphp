<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_compose', 'async');

// Run with ASYNC_WORKERS=1. await_any must suspend cooperatively so the single
// worker can run the nested promises. The first nested promise rejects; any
// must skip it and resolve with the fulfilled one.
$outer = oxphp_async(function (): int {
    $bad = oxphp_async(function (): int {
        throw new \RuntimeException('nope');
    });
    $good = oxphp_async(fn (): int => 7);
    $r = oxphp_async_await_any([$bad, $good]);
    return $r['value']; // first fulfilled wins
});

$result = oxphp_async_await($outer, 3.0);

$t->assertSame(
    'await_any composes inside a fiber, skipping the rejection',
    $result,
    7
);

$t->done();
