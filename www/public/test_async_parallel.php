<?php
// Параллельное выполнение: dispatch N задач, await_all, замеряем время.
// Если задачи идут параллельно, общее время ≈ max(sleep), а не sum(sleep).
header('Content-Type: application/json');

$start = hrtime(true);

$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(function(int $id): array {
        $t0 = hrtime(true);
        usleep(200_000); // 200ms каждая
        $t1 = hrtime(true);
        return ['worker' => $id, 'ms' => (int)(($t1 - $t0) / 1_000_000)];
    }, $i);
}

$results = oxphp_async_await_all($promises);
$elapsed_ms = (int)((hrtime(true) - $start) / 1_000_000);

// Если параллельно: ~200ms. Если последовательно: ~800ms.
$parallel = $elapsed_ms < 600;

echo json_encode([
    'test'       => 'parallel_execution',
    'elapsed_ms' => $elapsed_ms,
    'results'    => array_values($results),
    'parallel'   => $parallel,
    'pass'       => $parallel,
]);
