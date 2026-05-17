<?php
// Requires SHARED_LOCK_DIAGNOSTICS=strict + ASYNC_WORKERS >= 2.
// Triggers AB-BA deadlock: worker 1 grabs $a then waits for $b,
// worker 2 grabs $b then waits for $a. The detector (50 ms poll)
// breaks the cycle by signalling one side via DeadlockException.
header('Content-Type: text/plain');

$a = new OxPHP\Shared\Mutex(0);
$b = new OxPHP\Shared\Mutex(0);

$p1 = oxphp_async(function() use ($a, $b) {
    try {
        $a->withLockTimeout(function() use ($b) {
            usleep(300000); // 300 ms — give the 50 ms poll multiple windows
            $b->withLockTimeout(fn() => 1, 5000);
        }, 5000);
    } catch (OxPHP\Shared\DeadlockException $e) {
        return 'dl1';
    }
    return 'ok1';
});

$p2 = oxphp_async(function() use ($a, $b) {
    try {
        $b->withLockTimeout(function() use ($a) {
            usleep(300000);
            $a->withLockTimeout(fn() => 1, 5000);
        }, 5000);
    } catch (OxPHP\Shared\DeadlockException $e) {
        return 'dl2';
    }
    return 'ok2';
});

$results = oxphp_async_await_all([$p1, $p2]);

$deadlock_signals = array_filter($results, fn($r) => str_starts_with($r, 'dl'));
if (count($deadlock_signals) < 1) {
    echo "FAIL: expected at least one DeadlockException signal; got: "
         . implode(',', $results) . "\n"; exit;
}

echo "OK\n";
