<?php

declare(strict_types=1);

// Shared reader for the recycle counters every probe in this group asserts on.
//
// require_once, not require: PHP_WORKERS=1 here, so every test in the profile
// runs on the same persistent worker until it is recycled, and a bare require
// re-declares this.

/**
 * Worker-mode recycle counters as the server exposes them.
 *
 * The server writes no by-reason line for a counter that is still zero, so
 * an absent line means zero rather than missing data — which is why the
 * always-written total is read alongside them as the witness that the
 * worker-mode block was exposed at all.
 *
 * @return array{total: int, scheduled: int, max_memory: int, error: int}|null
 *         null when /metrics is unreachable or carries no worker-mode block
 */
function recycle_counts(): ?array
{
    // A timeout of its own: default_socket_timeout is 60 s, and an internal
    // listener that stopped answering would hold this profile's single worker
    // long past the point where the runner had given up.
    $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
    $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
    if (!is_string($body)) {
        return null;
    }

    if (!preg_match('/^oxphp_worker_recycles_total (\d+)$/m', $body, $total)) {
        return null;
    }

    $counts = ['total' => (int)$total[1], 'scheduled' => 0, 'max_memory' => 0, 'error' => 0];
    foreach (['scheduled', 'max_memory', 'error'] as $reason) {
        if (preg_match('/^oxphp_worker_recycles_by_reason_total\{reason="' . $reason . '"\} (\d+)$/m', $body, $m)) {
            $counts[$reason] = (int)$m[1];
        }
    }

    return $counts;
}
