<?php

declare(strict_types=1);

/*
 * Shared\Channel fan-in throughput benchmark (non-worker async pool). Not a
 * registered test.
 *
 * Top-level oxphp_async() dispatches producers and consumers onto the async
 * worker pool (ASYNC_WORKERS>0), where recv()/send() are thread-blocking.
 * Consumers outnumber producers on a small-capacity channel to exercise the
 * contended drain. (For the cooperative-fiber recv path, run bench_worker.php
 * under worker mode instead.)
 *
 * Query params:
 *   n       total messages          (default 1_000_000)
 *   p       producers               (default 4)
 *   c       consumers               (default 16)
 *   cap     channel capacity        (default 1)
 *   payload "scalar" | "array"      (default scalar)
 */

$n       = max(1, (int)($_GET['n']   ?? 1_000_000));
$p       = max(1, (int)($_GET['p']   ?? 4));
$c       = max(1, (int)($_GET['c']   ?? 16));
$cap     = max(1, (int)($_GET['cap'] ?? 1));
$payload = ($_GET['payload'] ?? 'scalar') === 'array' ? 'array' : 'scalar';

$ch  = new OxPHP\Shared\Channel($cap);
$per = intdiv($n, $p);
$realTotal = $per * $p;

$t0 = hrtime(true);

$producers = [];
for ($i = 0; $i < $p; $i++) {
    $producers[] = oxphp_async(function () use ($ch, $per, $payload): int {
        $msg = $payload === 'array' ? [1, 2, 3, 'k' => 'v'] : 7;
        for ($j = 0; $j < $per; $j++) {
            $ch->send($msg);
        }
        return $per;
    });
}

$consumers = [];
for ($i = 0; $i < $c; $i++) {
    $consumers[] = oxphp_async(function () use ($ch): int {
        $got = 0;
        for (;;) {
            $r = $ch->recv();
            if ($r->isClosed()) {
                break;
            }
            $got++;
        }
        return $got;
    });
}

// Wait for every send to land, then close so idle consumers see Closed.
oxphp_async_await_all($producers);
$ch->close();
$consCounts = oxphp_async_await_all($consumers);

$t1 = hrtime(true);
$wallNs   = $t1 - $t0;
$received = array_sum($consCounts);
$rate     = $wallNs > 0 ? ($received / ($wallNs / 1e9)) : 0;

header('Content-Type: text/plain');
printf(
    "sent=%d received=%d producers=%d consumers=%d cap=%d payload=%s\n",
    $realTotal, $received, $p, $c, $cap, $payload
);
printf("wall_ns=%d wall_ms=%.1f throughput_msg_s=%.0f\n", $wallNs, $wallNs / 1e6, $rate);
