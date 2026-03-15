<?php

/**
 * Async promises API endpoint.
 *
 * Demonstrates oxphp_async() / oxphp_async_await_all() / oxphp_async_await_any().
 * Query params:
 *   ?mode=parallel  — dispatch 4 tasks, await all (default)
 *   ?mode=race      — dispatch 3 tasks, await fastest
 *   ?mode=compute   — fan-out array processing across workers
 */

if (!function_exists('oxphp_async')) {
    json_response(503, [
        'error'   => 'Async not available',
        'detail'  => 'oxphp_async() requires the OxPHP server with ASYNC_WORKERS enabled.',
    ]);
    return;
}

$mode = $_GET['mode'] ?? 'parallel';
$start = microtime(true);

switch ($mode) {
    case 'parallel':
        $tasks = [];
        for ($i = 0; $i < 4; $i++) {
            $sleep_ms = random_int(100, 500);
            $tasks[] = [
                'id'       => $i + 1,
                'sleep_ms' => $sleep_ms,
                'promise'  => oxphp_async(function () use ($sleep_ms, $i) {
                    $t0 = microtime(true);
                    usleep($sleep_ms * 1000);
                    return [
                        'task'     => $i + 1,
                        'sleep_ms' => $sleep_ms,
                        'actual_ms' => round((microtime(true) - $t0) * 1000, 1),
                    ];
                }),
            ];
        }

        $promises = array_column($tasks, 'promise');
        $results = oxphp_async_await_all($promises);
        $wall_ms = round((microtime(true) - $start) * 1000, 1);

        json_response(200, [
            'mode'        => 'parallel',
            'dispatched'  => count($tasks),
            'results'     => $results,
            'wall_ms'     => $wall_ms,
            'sequential_ms' => array_sum(array_column($tasks, 'sleep_ms')),
            'speedup'     => round(array_sum(array_column($tasks, 'sleep_ms')) / max($wall_ms, 0.1), 2) . 'x',
        ]);
        break;

    case 'race':
        $durations = [400, 200, 300];
        $promises = [];
        foreach ($durations as $idx => $ms) {
            $promises[] = oxphp_async(function () use ($ms, $idx) {
                $t0 = microtime(true);
                usleep($ms * 1000);
                return [
                    'task'      => $idx + 1,
                    'sleep_ms'  => $ms,
                    'actual_ms' => round((microtime(true) - $t0) * 1000, 1),
                ];
            });
        }

        $winner = oxphp_async_await_any($promises);
        $wall_ms = round((microtime(true) - $start) * 1000, 1);

        json_response(200, [
            'mode'       => 'race',
            'dispatched' => count($durations),
            'durations'  => $durations,
            'winner'     => $winner,
            'wall_ms'    => $wall_ms,
        ]);
        break;

    case 'compute':
        $source = range(1, 1000);
        $chunk_size = (int)ceil(count($source) / 4);
        $chunks = array_chunk($source, $chunk_size);
        $promises = [];

        foreach ($chunks as $idx => $chunk) {
            $promises[] = oxphp_async(function () use ($chunk, $idx) {
                $t0 = microtime(true);
                // Simulate compute: sum of squares
                $result = 0;
                foreach ($chunk as $n) {
                    $result += $n * $n;
                }
                return [
                    'chunk'     => $idx + 1,
                    'size'      => count($chunk),
                    'result'    => $result,
                    'actual_ms' => round((microtime(true) - $t0) * 1000, 2),
                ];
            });
        }

        $results = oxphp_async_await_all($promises);
        $total = array_sum(array_column($results, 'result'));
        $wall_ms = round((microtime(true) - $start) * 1000, 1);

        json_response(200, [
            'mode'       => 'compute',
            'dispatched' => count($chunks),
            'input_size' => count($source),
            'results'    => $results,
            'total'      => $total,
            'wall_ms'    => $wall_ms,
        ]);
        break;

    default:
        json_response(400, [
            'error' => 'Unknown mode',
            'valid' => ['parallel', 'race', 'compute'],
        ]);
}
