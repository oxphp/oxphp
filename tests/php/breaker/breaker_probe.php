<?php

declare(strict_types=1);

// Shared reader for the two numbers every probe in this group asserts on.
//
// The worker's own view (requestCount()) says how many requests the worker
// answering right now has served; the server's view (/metrics) says how many
// workers have been recycled and why. Together they separate "the breaker
// tripped" from "a worker died of something else": a fresh worker with no
// recorded recycle is a crash, and a recorded recycle under reason="error" is
// this breaker and nothing else in the profile.
//
// require_once, not require: PHP_WORKERS=1 here, so every test in the profile
// runs on the same persistent worker and a bare require re-declares these.

/**
 * Worker-mode recycle counters as the server exposes them.
 *
 * Prometheus exposition omits a by-reason line whose counter is still zero, so
 * an absent reason="error" line means zero rather than missing data — which is
 * why the always-written total is read alongside it as the witness that the
 * worker-mode block was exposed at all.
 *
 * @return array{total: int, error: int}|null null when /metrics is unreachable
 *                                            or carries no worker-mode block
 */
function breaker_recycles(): ?array
{
    // With a timeout of its own: default_socket_timeout is 60 s, four times the
    // runner's patience, so an internal listener that stopped answering would
    // hold this profile's single worker long past the point where the runner had
    // given up — on this request and on every line after it. Failing the read
    // costs one line instead.
    $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
    $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
    if (!is_string($body)) {
        return null;
    }

    if (!preg_match('/^oxphp_worker_recycles_total (\d+)$/m', $body, $total)) {
        return null;
    }

    $error = 0;
    if (preg_match('/^oxphp_worker_recycles_by_reason_total\{reason="error"\} (\d+)$/m', $body, $m)) {
        $error = (int)$m[1];
    }

    return ['total' => (int)$total[1], 'error' => $error];
}
