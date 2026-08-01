<?php

/**
 * The inner request of the cross-request tests.
 *
 * Runs while the outer request's fiber is parked on a socket read, on the same
 * worker thread. Reports what the loop state looked like from here, then leaves
 * a mark of its own behind so the outer request can check whether its own state
 * survived a concurrent request.
 */

declare(strict_types=1);

require_once __DIR__ . '/revolt_bootstrap.php';

$probe = revolt_probe();

revolt_shared_local()->set('inner');

header('Content-Type: application/json');
echo json_encode([
    'marker' => 'inner-done',
    'suspension_id' => $probe['suspension_id'],
    'local_seen' => $probe['local_seen'],
    'is_userland_fiber' => $probe['is_userland_fiber'],
], JSON_UNESCAPED_SLASHES);
