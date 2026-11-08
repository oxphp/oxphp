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
     * @return array{opcache_enabled: bool, restart_pending: bool, manual_restarts: int, last_restart_time: int}
     */
    function ox_opcache_state(): array
    {
        $s = opcache_get_status(false);
        if (!is_array($s)) {
            return ['opcache_enabled' => false, 'restart_pending' => false, 'manual_restarts' => -1, 'last_restart_time' => 0];
        }
        return [
            'opcache_enabled' => (bool) $s['opcache_enabled'],
            'restart_pending' => (bool) $s['restart_pending'],
            'manual_restarts' => (int) $s['opcache_statistics']['manual_restarts'],
            'last_restart_time' => (int) $s['opcache_statistics']['last_restart_time'],
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

    /**
     * Start load_pump.php in the background against `$path`, wait until its
     * requests are out, and return the base of its marker files.
     */
    function ox_start_load(string $path, int $parallel, float $spread, float $seconds, string $name): string
    {
        $base = sys_get_temp_dir() . '/oxphp_opcacherestart_' . $name;
        foreach (['started', 'finished', 'stop', 'before'] as $s) {
            if (is_file("$base.$s")) {
                unlink("$base.$s");
            }
        }
        exec(sprintf(
            '/usr/local/bin/php %s %s %d %F %F %s > /dev/null 2>&1 &',
            escapeshellarg(__DIR__ . '/load_pump.php'),
            escapeshellarg($path),
            $parallel,
            $spread,
            $seconds,
            escapeshellarg($base),
        ));
        $deadline = microtime(true) + 5.0;
        while (!is_file("$base.started") && microtime(true) < $deadline) {
            usleep(20000);
            clearstatcache();
        }
        return $base;
    }

    /** Stop a load started with ox_start_load and return its tally. */
    function ox_stop_load(string $base): ?array
    {
        touch("$base.stop");
        $deadline = microtime(true) + 5.0;
        while (!is_file("$base.finished") && microtime(true) < $deadline) {
            usleep(20000);
            clearstatcache();
        }
        return is_file("$base.finished") ? json_decode((string) file_get_contents("$base.finished"), true) : null;
    }
}
