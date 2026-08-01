<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('foreign_suspend_is_refused', 'fibers');

// Suspending the request fiber from userland hands control to a scheduler that
// has no way to resume it — nothing in the request is waiting on anything, so
// the request would park forever. It must fail immediately and visibly instead.
$t->assertThrows(
    'Fiber::suspend() on the request fiber throws',
    static fn () => \Fiber::suspend(),
    \FiberError::class
);

// A caught refusal is not a licence to try again: the second attempt is refused
// on the same terms rather than parking the request the first one saved.
$t->assertThrows(
    'a second Fiber::suspend() is refused too',
    static fn () => \Fiber::suspend(),
    \FiberError::class
);

// And the request survives: the refusal unwinds the suspend attempt, not the
// request.
$t->assertNotNull('the request is still running after the refusal', \Fiber::getCurrent());
$t->assertTrue('the request can still work', oxphp_is_worker());

// Including the parts of it that need the scheduler — the refusal left the
// fiber usable, not merely alive.
$before = microtime(true);
oxphp_sleep(0.02);
$t->assertGreaterThan('the request can still park and be resumed', microtime(true) - $before, 0.015);

$t->done();
