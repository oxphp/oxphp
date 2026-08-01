<?php

/**
 * RED. Fails on current behaviour.
 *
 * Revolt\EventLoop\FiberLocal keys its storage on \Fiber::getCurrent() and falls
 * back to one process-wide dummy fiber when there is none (FiberLocal::getFiberStorage()).
 * Worker-mode request fibers are not userland \Fiber objects, so every request on
 * the worker lands in that one slot — the storage amphp uses for per-operation
 * context becomes shared mutable state between unrelated requests.
 *
 * Two directions are asserted, because the leak runs both ways:
 *   - a concurrent request must not read what this request wrote;
 *   - this request's own value must survive that concurrent request.
 */

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/revolt_bootstrap.php';

$t = new TestCase('fiberlocal_not_leaked_across_requests', 'revolt');

revolt_shared_local()->set('outer');
$t->assertSame('own value reads back before the concurrent request', (string) revolt_shared_local()->get(), 'outer');

// Parks this fiber; the inner request reads the local, then sets it to 'inner'.
$body = revolt_inner_request('/tests/revolt/fixture_inner_probe.php');
$inner = json_decode($body, true);

$t->assertTrue('inner request was served on the same worker', is_array($inner));

$t->assertNotEqual(
    "a concurrent request must not see this request's FiberLocal value",
    $inner['local_seen'] ?? null,
    'outer'
);

$t->assertSame(
    'own FiberLocal value survives a concurrent request',
    (string) revolt_shared_local()->get(),
    'outer'
);

$t->done();
