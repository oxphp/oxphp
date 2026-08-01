<?php

/**
 * The traditional-mode half of the fiber-identity boundary.
 *
 * Only worker mode multiplexes requests as fibers. A traditional-mode request
 * runs on the thread's main context, so \Fiber::getCurrent() is null inside one
 * and Revolt classifies it as {main} — which is what keeps EventLoop::run()
 * usable here. The worker-mode profile asserts the opposite of every line below;
 * together the two record exactly where the boundary falls, so a future change
 * that moves it cannot do so quietly.
 */

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once '/var/www/html/revolt/autoload.php';

use Revolt\EventLoop;

$t = new TestCase('traditional_request_runs_the_loop', 'revoltclassic');

$t->assertFalse('worker mode is not active', oxphp_is_worker());
$t->assertNull('no userland fiber is current inside the request', \Fiber::getCurrent());

$ran = [];
EventLoop::defer(static function () use (&$ran): void {
    $ran[] = 'defer';
});
EventLoop::delay(0.01, static function () use (&$ran): void {
    $ran[] = 'delay';
});

// Does not throw here: AbstractDriver::run()'s "Can't call run() within a fiber"
// guard only trips when the caller is a fiber, which a traditional-mode request
// is not.
EventLoop::run();

$t->assertEqual('the event loop ran both callbacks', $ran, ['defer', 'delay']);

$t->done();
