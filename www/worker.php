<?php
/**
 * OxPHP Worker Mode Demo
 *
 * This file is loaded via WORKER_FILE env var. The application boots once,
 * then oxphp_worker() loops internally — the handler is called for each HTTP
 * request with fresh $_GET, $_POST, $_SERVER, etc.
 *
 * Usage: WORKER_FILE=/var/www/html/worker.php
 */

// ── Application bootstrap (runs once per worker thread) ──────────

$counter = 0;
$boot_time = microtime(true);

// ── Request handler (called per HTTP request) ────────────────────

oxphp_worker(function () use (&$counter, $boot_time) {
    $counter++;

    // Minimal response for benchmarking — same payload as a typical bench_heavy.php
    $path = $_SERVER['REQUEST_URI'] ?? '/';
    if ($path === '/bench') {
        header('Content-Type: text/plain');
        echo "OK $counter";
        return;
    }

    header('Content-Type: application/json');

    echo json_encode([
        'message'        => 'Hello from worker mode!',
        'request_number' => $counter,
        'method'         => $_SERVER['REQUEST_METHOD'] ?? 'unknown',
        'uri'            => $path,
        'query'          => $_GET,
        'worker_id'      => oxphp_worker_id(),
        'request_id'     => oxphp_request_id(),
        'pid'            => getmypid(),
        'boot_time'      => $boot_time,
        'uptime_sec'     => round(microtime(true) - $boot_time, 3),
        'memory_mb'      => round(memory_get_usage(true) / 1024 / 1024, 2),
    ], JSON_PRETTY_PRINT);
});

// ── Graceful shutdown (runs after oxphp_worker() returns) ────────

// Reached on graceful shutdown, max_requests, or max_memory limit.
// Use this for cleanup: close persistent connections, flush caches, etc.
