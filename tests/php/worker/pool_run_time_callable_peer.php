<?php

declare(strict_types=1);

// Requested by test_pool_factory_reaches_scope_through_a_run_time_callable_on_a_peer
// while that test holds its own worker thread, so this runs on the other one.
// Every worker thread declares the constant, as a bootstrap file would; only
// the test's includes the file with the factory.
header('Content-Type: application/json');

\defined('WORKER_RUN_TIME_CALLABLE') || \define('WORKER_RUN_TIME_CALLABLE', 'static::make');

$out = ['helper_here' => function_exists('worker_run_time_callable_helper')];
try {
    $pool = \OxPHP\Shared\Registry::pool(
        'worker-run-time-callable-pool',
        static fn () => throw new \LogicException('the test did not register the pool'),
    );
    $handle = $pool->acquire();
    $out['result'] = $handle->get()->v;
    $handle->release();
} catch (\Throwable $e) {
    $out['result'] = $e::class . ': ' . $e->getMessage();
    $previous = $e->getPrevious();
    $out['previous'] = $previous === null ? null : $previous::class . ': ' . $previous->getMessage();
}
echo json_encode($out);
