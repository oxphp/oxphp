<?php

declare(strict_types=1);

// Helpers shared by the opcacherestart and opcacherestartworker tests. Guarded,
// because a worker keeps the functions an earlier request declared.

if (!function_exists('ox_opcache_state')) {
    /**
     * What OPcache says about the calling request and the shared memory:
     * whether this request runs with the cache (ZCG(accelerator_enabled), set
     * once at its startup), whether a restart is pending, and how many restarts
     * opcache_reset() has run so far.
     *
     * @return array{opcache_enabled: bool, restart_pending: bool, manual_restarts: int}
     */
    function ox_opcache_state(): array
    {
        $s = opcache_get_status(false);
        if (!is_array($s)) {
            return ['opcache_enabled' => false, 'restart_pending' => false, 'manual_restarts' => -1];
        }
        return [
            'opcache_enabled' => (bool) $s['opcache_enabled'],
            'restart_pending' => (bool) $s['restart_pending'],
            'manual_restarts' => (int) $s['opcache_statistics']['manual_restarts'],
        ];
    }

    /**
     * GET a fixture from this server and decode its JSON body; null when the
     * request failed or the body is not JSON.
     */
    function ox_get_json(string $path, float $timeout = 10.0): ?array
    {
        try {
            $body = file_get_contents(
                'http://127.0.0.1' . $path,
                false,
                stream_context_create(['http' => ['timeout' => $timeout, 'ignore_errors' => true]]),
            );
        } catch (\Throwable) {
            return null;
        }
        if (!is_string($body)) {
            return null;
        }
        $data = json_decode($body, true);
        return is_array($data) ? $data : null;
    }

    /**
     * Ask fixture_status.php until `$done` accepts an answer or `$seconds`
     * pass; returns the last answer.
     */
    function ox_poll_state(callable $done, float $seconds): ?array
    {
        $deadline = microtime(true) + $seconds;
        $last = null;
        do {
            $last = ox_get_json('/tests/opcacherestart/fixture_status.php', 5.0);
            if ($last !== null && $done($last)) {
                return $last;
            }
            usleep(200000);
        } while (microtime(true) < $deadline);
        return $last;
    }

    /**
     * Ask fixture_task_state.php until its two tasks, run on two different
     * pool threads, both report `$enabled` for the cache, or `$seconds` pass;
     * whether they did.
     */
    function ox_poll_tasks(bool $enabled, float $seconds): bool
    {
        $deadline = microtime(true) + $seconds;
        do {
            $tasks = ox_get_json('/tests/opcacherestart/fixture_task_state.php', 4.0);
            if (is_array($tasks) && count($tasks) === 2
                && ($tasks[0]['thread'] ?? null) !== ($tasks[1]['thread'] ?? null)
                && ($tasks[0]['opcache_enabled'] ?? null) === $enabled
                && ($tasks[1]['opcache_enabled'] ?? null) === $enabled
            ) {
                return true;
            }
            usleep(100000);
        } while (microtime(true) < $deadline);
        return false;
    }
}
