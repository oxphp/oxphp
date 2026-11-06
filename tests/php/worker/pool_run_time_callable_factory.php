<?php

declare(strict_types=1);

// Included by test_pool_factory_reaches_scope_through_a_run_time_callable_on_a_peer
// on its own worker thread only. The factory reads a callable from a global
// constant, which the check made before the call does not follow.

function worker_run_time_callable_helper(int $base = 0): int
{
    return $base + 11;
}

const WORKER_RUN_TIME_CALLABLE_CALL_LINE = __LINE__ + 5;
final class WorkerRunTimeCallableMaker
{
    public static function make(): object
    {
        return (object) ['v' => worker_run_time_callable_helper()];
    }

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(WORKER_RUN_TIME_CALLABLE);
        } finally {
            \restore_error_handler();
        }
    }
}

return [WorkerRunTimeCallableMaker::class, 'factory'];
