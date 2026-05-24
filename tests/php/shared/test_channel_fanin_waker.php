<?php
/**
 * Channel — fan-in over the fiber-waker recv path under contention.
 *
 * Regression for the case where recv() is suspended via the cooperative
 * fiber waker (consumer = the *main request fiber*, so recv parks on a
 * synthetic promise) while producers run on the async pool and deliver
 * cross-thread sends. This is distinct from
 * test_channel_fiber_producer_consumer (where the consumer is itself an
 * oxphp_async pool fiber and recv is thread-blocking).
 *
 * It guards two defects that turned this path unusable under load:
 *   1. A waker-path recvTimeout deadline must return RecvResult::Timeout,
 *      never a fatal "fiber waker raised exception" — the failure travels
 *      through bridge async-exception TLS, not EG(exception).
 *   2. A producer depositing into the buffer on the slow path must hand
 *      the item to a parked recv-waiter, or the consumer crawls (~150ms
 *      per recv) and overruns its deadline.
 *
 * cap=1 + no pacing maximises park/wake contention so both show up fast.
 *
 * Query params (optional): n total messages, p producers.
 *
 * Worker-mode only: needs the persistent fiber scheduler. Skips elsewhere.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$n = max(1, (int)($_GET['n'] ?? 2000));
$p = max(1, (int)($_GET['p'] ?? 4));
$per = intdiv($n, $p);
$n = $per * $p; // exact division so got === sent is unambiguous

$ch = new OxPHP\Shared\Channel(1); // cap=1 → every send contends a parked recv

// Producers run on the async pool (OS threads). No pacing: full speed.
// Each returns how many sends actually landed, so the harness can tell a
// send-side timeout apart from a recv-side loss.
$producers = [];
for ($i = 0; $i < $p; $i++) {
    $producers[] = oxphp_async(function () use ($ch, $per) {
        $ok = 0;
        for ($j = 0; $j < $per; $j++) {
            // sendTimeout (generous) so a transiently-full channel under
            // cap=1 retries rather than dropping.
            $r = $ch->sendTimeout(1, 5000);
            if ($r->isOk()) {
                $ok++;
            }
        }
        return $ok;
    });
}

// Consumer = the main request fiber → recv suspends through the waker.
$got = 0;
$sum = 0;
// Hard wall-clock budget. Kept under the test harness's 15s curl timeout
// so a stuck run reports a clean FAIL rather than a truncated response.
$deadline = microtime(true) + 12.0;

$stuck = false;
try {
    while ($got < $n) {
        $r = $ch->recvTimeout(200);
        if ($r->isOk()) {
            $got++;
            $sum += $r->value();
            continue;
        }
        // A bare Timeout mid-run is normal (consumer briefly out-paces
        // producers). Only the wall-clock budget bounds the loop.
        if (microtime(true) > $deadline) {
            $stuck = true;
            break;
        }
    }
} catch (\Throwable $e) {
    echo "FAIL: recv threw " . get_class($e) . ": " . $e->getMessage() . "\n";
    return;
}

// Collect how many sends actually landed (diagnostic: send-timeout vs
// recv-loss). Producers have all finished by now (got === n) or the
// consumer gave up (stuck), in which case they are done sending too.
$sent = 0;
try {
    foreach ($producers as $pid) {
        $sent += oxphp_async_await($pid);
    }
} catch (\Throwable $e) {
    echo "FAIL: producer threw " . get_class($e) . ": " . $e->getMessage() . "\n";
    return;
}

if ($stuck) {
    echo "FAIL: stuck — got=$got sent=$sent of $n after 12s "
        . "(" . ($sent < $n ? "send-side timeouts" : "recv-side loss") . ")\n";
    return;
}

if ($got !== $n || $sum !== $n) {
    echo "FAIL: expected got=$n sum=$n, got got=$got sum=$sum (sent=$sent)\n";
    return;
}

echo "OK\n";
