<?php

declare(strict_types=1);

/*
 * Shared\Channel throughput benchmark for worker mode. Not a registered test.
 *
 * Run as the worker entry script (WORKER_MODE_ENABLED=true,
 * ENTRY_FILE=/var/www/html/bench_worker.php) with an async pool
 * (ASYNC_WORKERS>0). Two modes:
 *
 *   GET /?n=N[&payload=array]
 *       One producer + one consumer, both dispatched via oxphp_async() onto
 *       the async pool. Measures straight producer/consumer throughput.
 *
 *   GET /?op=fanin&n=N[&p=4][&pace=US][&to=MS][&payload=array]
 *       Fan-in onto the request fiber: `p` producers on the async pool feed a
 *       single consumer that is THIS request fiber. recv() takes its fiber
 *       branch only when called from the worker request body (the scheduler
 *       fiber); pool closures would thread-block instead. With cap=1 and
 *       optional per-send pacing the consumer parks on an empty channel and is
 *       woken by a cross-thread send — exercising the recv waker path. `to=0`
 *       uses recv() (no deadline); `to>0` uses recvTimeout($to).
 *
 * Channel capacity is BENCH_CAP (default 1; cap=1 maximises park/wake).
 */

$cap = (int) (getenv('BENCH_CAP') ?: 1);

oxphp_worker(function () use ($cap) {
    header('Content-Type: text/plain');

    if (!oxphp_is_worker()) {
        echo "skip: not worker mode\n";
        return;
    }

    $n = max(1, (int) ($_GET['n'] ?? 200000));
    $payloadArr = ($_GET['payload'] ?? 'scalar') === 'array';

    if (($_GET['op'] ?? '') === 'fanin') {
        $ch = new OxPHP\Shared\Channel($cap);
        $p = max(1, (int) ($_GET['p'] ?? 4));
        $per = intdiv($n, $p);
        $realTotal = $per * $p;
        $msg = $payloadArr ? [1, 2, 3, 'k' => 'v'] : 7;

        // Per-send pacing (microseconds): with pacing the single fiber consumer
        // wins the race to an empty channel and parks, so the producer's send
        // hands off to the waiter (waker path). pace=0 lets producers keep the
        // buffer full, so recv mostly takes the buffered-hit path instead.
        $pace = max(0, (int) ($_GET['pace'] ?? 0));

        $producers = [];
        for ($i = 0; $i < $p; $i++) {
            $producers[] = oxphp_async(function () use ($ch, $per, $msg, $pace): int {
                for ($j = 0; $j < $per; $j++) {
                    $ch->send($msg);
                    if ($pace > 0) {
                        usleep($pace);
                    }
                }
                return $per;
            });
        }

        // Consumer = this request fiber. recvTimeout retries on timeout so a
        // slow wakeup can't deadlock; a wall-clock guard bounds the drain.
        $to = (int) ($_GET['to'] ?? 200);
        $t0 = hrtime(true);
        $got = 0;
        $sum = 0;       // checksum for scalar payloads (each value === 7)
        $badVal = 0;    // values that didn't match the expected payload
        $excCount = 0;  // recv exceptions, counted and skipped so the run finishes
        $guardNs = 30 * 1e9;
        while ($got < $realTotal && (hrtime(true) - $t0) < $guardNs) {
            try {
                $r = $to > 0 ? $ch->recvTimeout($to) : $ch->recv();
            } catch (\Throwable $e) {
                $excCount++;
                continue;
            }
            if ($r->isOk()) {
                $got++;
                $v = $r->value();
                if ($payloadArr ? ($v !== [1, 2, 3, 'k' => 'v']) : ($v !== 7)) {
                    $badVal++;
                }
                if (!$payloadArr) {
                    $sum += $v;
                }
            }
        }
        $wall = hrtime(true) - $t0;
        $ch->close();
        oxphp_async_await_all($producers);
        printf(
            "fanin n=%d got=%d p=%d cap=%d payload=%s wall_ms=%.1f throughput_msg_s=%.0f sum=%d bad_values=%d exceptions=%d\n",
            $realTotal, $got, $p, $cap, $payloadArr ? 'array' : 'scalar',
            $wall / 1e6, $wall > 0 ? $got / ($wall / 1e9) : 0, $sum, $badVal, $excCount
        );
        return;
    }

    $ch = new OxPHP\Shared\Channel($cap);

    $producer = oxphp_async(function () use ($ch, $n, $payloadArr): int {
        $msg = $payloadArr ? [1, 2, 3, 'k' => 'v'] : 7;
        for ($i = 0; $i < $n; $i++) {
            $ch->send($msg);
        }
        $ch->close();
        return $n;
    });

    $consumer = oxphp_async(function () use ($ch): int {
        $got = 0;
        while (true) {
            $r = $ch->recv();
            if (!$r->isOk()) {
                break; // Closed + drained
            }
            $got++;
        }
        return $got;
    });

    $t0 = hrtime(true);
    oxphp_async_await($producer);
    $got = oxphp_async_await($consumer);
    $wall = hrtime(true) - $t0;

    printf(
        "n=%d got=%d cap=%d wall_ms=%.1f throughput_msg_s=%.0f\n",
        $n, $got, $cap, $wall / 1e6, $wall > 0 ? $got / ($wall / 1e9) : 0
    );
});
