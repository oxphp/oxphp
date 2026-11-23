<?php
// Worker-mode fixture for Scenario M of tests/graceful_drain.sh: a worker that,
// once oxphp_worker() returns, awaits tasks the async pool never starts. Two
// tasks keep both async worker threads busy without letting go of them (usleep
// does not suspend a task) until well after the server is told to stop, so the
// tasks queued behind them are still queued when the pool stops. The worker
// waits until the pool has stopped, then awaits each queued task in another
// way and logs what the await did. A task dispatched first answers before
// the busy ones start, and its answer must still be returned after the stop.
$boot = microtime(true);

$answered = oxphp_async(static fn (): int => 7);

$busy = static function () use ($boot): int {
    while (microtime(true) < $boot + 8.0) {
        usleep(50_000);
    }
    return 0;
};
oxphp_async($busy);
oxphp_async($busy);

$queued = [];
for ($i = 0; $i < 5; $i++) {
    $queued[] = oxphp_async(static fn (): int => 1);
}

oxphp_worker(static function (): void {
    echo 'up';
});

// The pool waits for the busy tasks before it stops.
while (microtime(true) < $boot + 11.0) {
    usleep(100_000);
}
$steps = [
    'await of a task that answered before the stop' => static fn (): mixed => oxphp_async_await($answered, 5.0),
    'await with a timeout' => static fn (): mixed => oxphp_async_await($queued[0], 5.0),
    'race with a timeout' => static fn (): mixed => oxphp_async_await_race([$queued[1]], 5.0),
    'all with a timeout' => static fn (): mixed => oxphp_async_await_all([$queued[2]], 5.0),
    'any without a timeout' => static fn (): mixed => oxphp_async_await_any([$queued[3]]),
    'await without a timeout' => static fn (): mixed => oxphp_async_await($queued[4]),
];
foreach ($steps as $name => $step) {
    try {
        $value = $step();
        error_log("drain-await: $name returned " . json_encode($value));
    } catch (\Throwable $e) {
        error_log("drain-await: $name threw " . $e::class . ': ' . $e->getMessage());
    }
}
