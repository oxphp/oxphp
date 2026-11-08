<?php

declare(strict_types=1);

// Whether the pool threads run their tasks with the cache: two tasks that each
// keep their thread busy long enough that the second lands on the other thread,
// each reporting what OPcache says on the thread it ran on.

require_once __DIR__ . '/lib.php';

header('Content-Type: application/json');
$task = static function (): array {
    $until = microtime(true) + 0.3;
    while (microtime(true) < $until) {
    }
    $s = opcache_get_status(false);
    return [
        'opcache_enabled' => is_array($s) && $s['opcache_enabled'],
        'thread' => oxphp_worker_id(),
    ];
};
$a = oxphp_async($task);
$b = oxphp_async($task);
echo json_encode([oxphp_async_await($a, 5.0), oxphp_async_await($b, 5.0)]);
