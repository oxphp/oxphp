<?php

declare(strict_types=1);

// Requested by test_pool_callback_on_peer_worker_copies_captures and
// test_pool_callback_made_from_a_function_on_peer_worker while the test holds
// its own worker thread, so this runs on the other one. ?step=acquire acquires
// from the pool the test registered under ?pool= twice, evicting the slot
// after each, and reports what each slot holds; ?step=collect collects garbage
// on this thread.
header('Content-Type: application/json');

if (($_GET['step'] ?? '') === 'collect') {
    echo json_encode(['collected' => gc_collect_cycles()]);
    return;
}

$out = [];
try {
    $pool = \OxPHP\Shared\Registry::pool(
        $_GET['pool'] ?? 'worker-captures-pool',
        static fn () => throw new \LogicException('the test did not register the pool'),
    );
    for ($i = 0; $i < 2; $i++) {
        $handle = $pool->acquire();
        $out[] = $handle->get()->v;
        $handle->release();
        $pool->evict();
    }
} catch (\Throwable $e) {
    $out[] = $e::class . ': ' . $e->getMessage();
}
echo json_encode(['result' => $out]);
