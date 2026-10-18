<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_timeout_waits_for_the_task_at_request_end', 'async');

// When a single `oxphp_async_await($p, $timeout)` blocks the thread — always the
// case outside worker mode — and gives up on its deadline, the task it abandons
// is cancelled and must still be drained at the end of the request, as the
// losers of await_race / await_any already are. That guarantee has two halves,
// pinned by two runs of the fixture:
//
//   * granted — a task that stops one second after the cancel is waited for: it
//     finishes before the client receives the response (`ended_at < resp_at`),
//     and the response is released once it has (`elapsed < 4`), not held to the
//     full budget;
//   * bounded — a task that keeps working eight seconds after the cancel holds
//     the response for the budget and no longer (`4 < elapsed < 7`). Premise,
//     read after the fact: that task really did outlive the bound
//     (`ended_at - t0 > 7`), so a wait without one would have broken
//     `elapsed < 7`.
//
// `ended_at` is the task's own wall-clock finish time and `t0` / `resp_at` are
// measured here; both processes share this host's clock, so they are
// comparable. A request end that does not wait for the task answers in about
// 0.2 s, which breaks `ended_at < resp_at` in the first run and `elapsed > 4`
// in the second.
//
// Each run is a separate request so its end can be observed from here; it
// needs a second PHP worker (PHP_WORKERS >= 2) while this one waits. Each run
// also waits for its task to finish — on the task's schedule, not the
// response's, so the premise is read however early the response came — and no
// task is left on the async pool.
$run = static function (string $id, int $linger) use ($t): array {
    $base = sys_get_temp_dir() . "/oxphp_awaittmo_{$id}";

    $t0 = microtime(true);
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("fixture socket connected (linger {$linger} s)", $sock !== false);
    if ($sock === false) {
        $t->done();
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/async/fixture_await_timeout_orphan.php?id={$id}&linger={$linger} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $resp_at = microtime(true);

    // The task ends about `0.2 + linger` seconds after `t0`.
    $deadline = $t0 + $linger + 4.0;
    while (!file_exists("$base.ended_at") && microtime(true) < $deadline) {
        usleep(20000);
        clearstatcache();
    }
    $ended_at = is_file("$base.ended_at") ? (float) file_get_contents("$base.ended_at") : 0.0;

    foreach (['started', 'ended_at'] as $s) {
        $f = "$base.$s";
        if (is_file($f)) {
            @unlink($f);
        }
    }
    return [$resp, $t0, $resp_at, $ended_at];
};

$id = bin2hex(random_bytes(6));

// Granted: the task stops one second after the cancel.
[$resp, $t0, $resp_at, $ended_at] = $run("{$id}a", 1);
$elapsed = $resp_at - $t0;

$t->assertContains('the single await timed out as expected', $resp, 'await-timed-out');
$t->assertGreaterThan('the task recorded a finish time', $ended_at, 0.0);
$t->assertLessThan('the request end waited for the timed-out task', $ended_at, $resp_at);
$t->assertLessThan('the response was released once the task stopped', $elapsed, 4.0);
$t->meta('granted_elapsed', round($elapsed, 3));
$t->meta('granted_ended_before_resp_by', round($resp_at - $ended_at, 3));

// Bounded: the task keeps working eight seconds after the cancel.
[$resp, $t0, $resp_at, $ended_at] = $run("{$id}b", 8);
$elapsed = $resp_at - $t0;

$t->assertContains('the single await timed out as expected (long task)', $resp, 'await-timed-out');
$t->assertGreaterThan('the request end waited for the long task', $elapsed, 4.0);
$t->assertLessThan('the wait stopped at the drain budget', $elapsed, 7.0);
$t->assertGreaterThan('the long task outlived the bound', $ended_at - $t0, 7.0);
$t->meta('bounded_elapsed', round($elapsed, 3));
$t->meta('bounded_task_ran_for', round($ended_at - $t0, 3));

$t->done();
