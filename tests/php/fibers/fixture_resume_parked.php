<?php

/**
 * The inner request of the foreign-resume test.
 *
 * Runs while the outer request's fiber is parked on a socket read, on the same
 * worker thread, and does to that fiber what a userland event loop holding a
 * \Fiber::getCurrent() would do: resume it, and throw into it. Reports what came
 * back from each attempt so the outer request can assert on it.
 */

declare(strict_types=1);

require_once __DIR__ . '/fiber_park_registry.php';

$outer = ParkedRequestFiber::get();

header('Content-Type: application/json');
echo json_encode([
    'marker' => 'inner-done',
    'found' => $outer !== null,
    'was_suspended' => $outer !== null && $outer->isSuspended(),
    'resume' => $outer === null ? 'no-fiber' : fiber_attempt(static fn () => $outer->resume('nudge')),
    'throw' => $outer === null ? 'no-fiber' : fiber_attempt(
        static fn () => $outer->throw(new \RuntimeException('nudge'))
    ),
    'still_suspended' => $outer !== null && $outer->isSuspended(),
], JSON_UNESCAPED_SLASHES);
