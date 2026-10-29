<?php

declare(strict_types=1);

// Runs fixture_orphan_child_at_parent_end.php as a separate request, so that
// the moment its response arrives can be set against what its tasks recorded,
// and returns both. Shared by the tests of a task that ends without awaiting a
// task it dispatched; they differ in profile and in what they assert.
//
// A child nothing cancels runs for 20 s, so the wait for its `ended_at` ends
// `$wait` seconds after the response instead: an absent finish time is the
// failure the tests report, not a hang.

if (!function_exists('oxphp_orphan_child_run')) {
    /**
     * @return array{resp: string, t0: float, resp_at: float, parent_done: float, cancelled_at: float, ended_at: float}
     */
    function oxphp_orphan_child_run(TestCase $t, string $mode, float $linger, float $wait): array
    {
        $id = bin2hex(random_bytes(6));
        $base = sys_get_temp_dir() . "/oxphp_orphanchild_{$id}";

        $t0 = microtime(true);
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        $t->assertTrue('fixture socket connected', $sock !== false);
        if ($sock === false) {
            $t->done();
        }
        stream_set_timeout($sock, 40);
        fwrite($sock, "GET /tests/async/fixture_orphan_child_at_parent_end.php"
            . "?id={$id}&linger={$linger}&mode={$mode} HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
        $resp = (string) stream_get_contents($sock);
        fclose($sock);
        $resp_at = microtime(true);

        $deadline = $resp_at + $wait;
        while (!file_exists("$base.ended_at") && microtime(true) < $deadline) {
            usleep(20000);
            clearstatcache();
        }

        $read = static fn (string $s): float
            => is_file("$base.$s") ? (float) file_get_contents("$base.$s") : 0.0;
        $run = [
            'resp' => $resp,
            't0' => $t0,
            'resp_at' => $resp_at,
            'parent_done' => $read('parent_done'),
            'cancelled_at' => $read('cancelled_at'),
            'ended_at' => $read('ended_at'),
        ];
        foreach (['started', 'parent_done', 'cancelled_at', 'ended_at'] as $s) {
            if (is_file("$base.$s")) {
                @unlink("$base.$s");
            }
        }

        $t->meta('elapsed', round($resp_at - $t0, 3));
        if ($run['cancelled_at'] > 0.0) {
            $t->meta('cancelled_after_parent', round($run['cancelled_at'] - $run['parent_done'], 3));
        }
        if ($run['ended_at'] > 0.0) {
            $t->meta('child_ended_minus_response', round($run['ended_at'] - $resp_at, 3));
        }

        return $run;
    }
}
