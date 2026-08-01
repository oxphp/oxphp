<?php

/**
 * RED. Fails on current behaviour.
 *
 * Revolt hands out one Suspension per execution context and keys the table on
 * \Fiber::getCurrent(), falling back to a single {main} key when there is no
 * userland fiber (AbstractDriver::getSuspension(): `$key = $fiber ?? $this->queueCallback`).
 *
 * A worker-mode request fiber is not a userland \Fiber — it is a raw
 * zend_fiber_context, and EG(active_fiber) is only set for \Fiber objects — so
 * \Fiber::getCurrent() returns null inside one and every concurrent request on
 * the worker collapses onto the {main} key. Two requests then share one
 * Suspension: whichever resumes it delivers into the other request.
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

$t->assertFalse(
    'the outer request is not a userland fiber (why the keys collapse)',
    $outer['is_userland_fiber']
);

$t->assertNotEqual(
    'a concurrent request gets its own Suspension, not this one',
    $inner['suspension_id'] ?? null,
    $outer['suspension_id']
);

$t->done();
