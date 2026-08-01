<?php

/**
 * Revolt hands out one Suspension per execution context and keys the table on
 * \Fiber::getCurrent(), falling back to a single {main} key when there is no
 * userland fiber (AbstractDriver::getSuspension(): `$key = $fiber ?? $this->queueCallback`).
 *
 * A worker-mode request runs as a real \Fiber, so \Fiber::getCurrent() names it
 * and each concurrent request on the worker gets its own key. Were it a bare
 * fiber context instead, EG(active_fiber) would stay unset, getCurrent() would
 * return null inside every request, and they would all collapse onto the single
 * {main} key — two requests sharing one Suspension, with whichever resumes it
 * delivering into the other.
 *
 * The assertion is that two requests multiplexed on one worker thread are handed
 * two different Suspension objects.
 */

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/revolt_bootstrap.php';

$t = new TestCase('suspension_not_shared_across_requests', 'revolt');

$outer = revolt_probe();

// Parks this fiber on the socket read (RUNTIME_HOOKS=1) and lets the worker
// pick the inner request up. PHP_WORKERS=1 makes that the same thread.
$body = revolt_inner_request('/tests/revolt/fixture_inner_probe.php');
$inner = json_decode($body, true);

$t->assertTrue('inner request was served on the same worker', is_array($inner));
$t->assertContains('inner request completed', $body, 'inner-done');

$t->assertTrue(
    'the outer request is a userland fiber (why the keys stay apart)',
    $outer['is_userland_fiber']
);

$t->assertNotEqual(
    'a concurrent request gets its own Suspension, not this one',
    $inner['suspension_id'] ?? null,
    $outer['suspension_id']
);

$t->done();
