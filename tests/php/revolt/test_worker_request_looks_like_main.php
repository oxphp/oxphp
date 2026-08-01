<?php

/**
 * Characterization, not RED — this one is expected to pass. It records the
 * mechanism the two RED tests in this group depend on, so that a failure there
 * can be read without re-deriving it.
 *
 * Two facts:
 *   - revolt/event-loop runs inside a worker-mode request. It is not broken
 *     today, which is why the isolation defect is quiet rather than loud.
 *   - it runs because \Fiber::getCurrent() returns null in a request fiber, so
 *     Revolt classifies the request as {main}: the guard in AbstractDriver::run()
 *     ("Can't call run() within a fiber") does not trip, and everything Revolt
 *     keys on the current fiber lands in the single {main} slot.
 *
 * The second fact is what the RED tests turn into a visible defect once two
 * requests are multiplexed on one worker thread.
 */

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/revolt_bootstrap.php';

use Revolt\EventLoop;

$t = new TestCase('worker_request_looks_like_main', 'revolt');

$t->assertTrue('worker mode is active', oxphp_is_worker());
$t->assertNull('no userland fiber is current inside a request fiber', \Fiber::getCurrent());

$ran = [];
EventLoop::defer(static function () use (&$ran): void {
    $ran[] = 'defer';
});
EventLoop::delay(0.01, static function () use (&$ran): void {
    $ran[] = 'delay';
});

// Would throw \Error("Can't call run() within a fiber") if the request fiber
// were a userland \Fiber.
EventLoop::run();

$t->assertEqual('the event loop ran both callbacks', $ran, ['defer', 'delay']);

$t->done();
