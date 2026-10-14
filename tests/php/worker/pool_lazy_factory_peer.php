<?php

declare(strict_types=1);

// Requested by test_pool_factory_from_file_peer_never_loaded while that test
// holds its own worker thread, so this runs on the other one: acquires from
// the pool the test registered and reports what came of it.
header('Content-Type: application/json');

$out = ['helper_here' => function_exists('worker_lazy_pool_helper')];
try {
    $pool = \OxPHP\Shared\Registry::pool(
        'worker-lazy-factory-pool',
        static fn () => throw new \LogicException('the test did not register the pool'),
    );
    $handle = $pool->acquire();
    $out['result'] = $handle->get()->v;
    $handle->release();
} catch (\Throwable $e) {
    $out['result'] = $e::class . ': ' . $e->getMessage();
}
echo json_encode($out);
