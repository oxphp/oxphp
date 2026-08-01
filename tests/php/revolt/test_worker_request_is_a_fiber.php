<?php

/**
 * Characterization. It records the mechanism the other two tests in this group
 * depend on, so that a failure there can be read without re-deriving it.
 *
 * A worker-mode request runs as a real \Fiber, so \Fiber::getCurrent() names it
 * and Revolt files everything it keys on the current fiber under a key unique to
 * that request. That is what the other two turn into visible isolation.
 *
 * It also pins the cost of the same fact: Revolt refuses to run its event loop
 * from inside a fiber (AbstractDriver::run(), "Can't call run() within a fiber"),
 * so a request cannot drive the loop itself. That guard is Revolt working as
 * designed under a fiber-per-request server — the loop belongs outside the
 * request, and inside one the scheduler is already driving it. Callbacks can
 * still be queued from a request; only run() is refused.
 */

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/revolt_bootstrap.php';

use Revolt\EventLoop;

$t = new TestCase('worker_request_is_a_fiber', 'revolt');

$t->assertTrue('worker mode is active', oxphp_is_worker());

$current = \Fiber::getCurrent();
$t->assertNotNull('a userland fiber is current inside a request', $current);
$t->assertInstanceOf('the request is a Fiber', $current, \Fiber::class);

// Queueing is unaffected — these register against the driver and return.
$ran = [];
EventLoop::defer(static function () use (&$ran): void {
    $ran[] = 'defer';
});
EventLoop::delay(0.01, static function () use (&$ran): void {
    $ran[] = 'delay';
});

$t->assertThrows(
    'EventLoop::run() from inside a request is refused',
    static fn () => EventLoop::run(),
    \Error::class
);

// The callbacks stay queued rather than running — nothing drove them.
$t->assertEmpty('a refused run() executed nothing', $ran);

$t->done();
