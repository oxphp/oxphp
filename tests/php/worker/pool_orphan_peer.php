<?php

declare(strict_types=1);

// Requested by test_pool_orphaned_when_its_creator_exits while that test holds
// its own worker thread, so this runs on the other one.
require_once __DIR__ . '/pool_orphan_lib.php';

header('Content-Type: application/json');

$name = (string) ($_GET['pool'] ?? '');
try {
    echo json_encode(match ($_GET['action'] ?? '') {
        'mint' => oxphp_pool_orphan_mint($name),
        'check' => oxphp_pool_orphan_check($name),
        default => ['error' => 'unknown action'],
    });
} catch (\Throwable $e) {
    echo json_encode(['error' => $e::class . ': ' . $e->getMessage()]);
}
