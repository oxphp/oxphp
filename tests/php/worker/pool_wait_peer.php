<?php

declare(strict_types=1);

// Requested by test_pool_creator_waits_for_a_peer_in_its_factory while that
// test holds its own worker thread, so this runs on the other one: makes a slot
// of the pool the test created, which runs the test's factory here.

use OxPHP\Shared\Registry;

header('Content-Type: application/json');

try {
    $pool = Registry::pool(
        (string) ($_GET['pool'] ?? ''),
        static fn () => throw new \LogicException('the test did not create the pool'),
    );
    $pool->acquire()->release();
    echo json_encode(['made' => true]);
} catch (\Throwable $e) {
    echo json_encode(['error' => $e::class . ': ' . $e->getMessage()]);
}
