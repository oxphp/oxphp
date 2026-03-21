<?php
$start = microtime(true);

$p1 = oxphp_async(function(): int {
    usleep(200_000); // 200ms
    return 1;
});

$p2 = oxphp_async(function(): int {
    usleep(200_000); // 200ms
    return 2;
});

$r1 = oxphp_async_await($p1);
$r2 = oxphp_async_await($p2);

$elapsed = microtime(true) - $start;

header('Content-Type: application/json');
echo json_encode([
    'results' => [$r1, $r2],
    'elapsed_ms' => round($elapsed * 1000),
    'worker_id' => oxphp_worker_id(),
]);
