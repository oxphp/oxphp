<?php

declare(strict_types=1);

// Shared staging for the requests whose clients leave while they are still
// waiting in the queue.
//
// The shape: this profile has one worker, and the request calling
// queued_cancel_stage() is what it is running. Three more requests are sent
// from here, over sockets of this request's own, so they can only wait in the
// queue behind it; then their sockets are closed, and the server sees three
// clients leave before any worker has reached their requests. What the worker
// does when it does reach them is up to the test that staged them.
//
// Each step is confirmed before the next one is taken, and confirmed from
// /metrics rather than assumed from timing, because a run that skipped one of
// them would pass for the wrong reason: requests that were never queued, or
// whose clients the server had not yet seen leave by the time a worker took
// them, run by design.
//
// require_once, not require, for the reason breaker_probe.php gives.

/** Where a victim that was run leaves its mark: one line per run. */
function queued_cancel_marker(): string
{
    return '/tmp/oxphp-breaker-queued-cancel-victim';
}

/** How many queued requests were run, according to their marks. */
function queued_cancel_victims_run(): int
{
    $marker = queued_cancel_marker();

    return is_file($marker) ? substr_count((string) file_get_contents($marker), "ran\n") : 0;
}

/**
 * The server-side numbers the staging and its assertions read.
 *
 * @return array{queue_depth: int, active_connections: int, handled: int}|null
 *         null when /metrics is unreachable or lacks one of them
 */
function queued_cancel_metrics(): ?array
{
    // With a timeout of its own, as breaker_recycles() has.
    $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
    $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
    if (!is_string($body)) {
        return null;
    }

    $read = [
        'queue_depth' => 'oxphp_queue_depth',
        'active_connections' => 'oxphp_active_connections',
        'handled' => 'oxphp_worker_requests_handled_total',
    ];
    $out = [];
    foreach ($read as $key => $name) {
        if (!preg_match('/^' . $name . ' (\d+)$/m', $body, $m)) {
            return null;
        }
        $out[$key] = (int) $m[1];
    }

    return $out;
}

/**
 * Polls /metrics until $until accepts a reading, for at most three seconds.
 *
 * usleep, not oxphp_sleep: this profile runs no hooks, so it blocks the worker
 * instead of yielding it, and the queued requests stay queued while this waits.
 *
 * @param callable(array{queue_depth: int, active_connections: int, handled: int}): bool $until
 */
function queued_cancel_wait(callable $until): bool
{
    $deadline = microtime(true) + 3.0;
    do {
        $m = queued_cancel_metrics();
        if ($m !== null && $until($m)) {
            return true;
        }
        usleep(20_000);
    } while (microtime(true) < $deadline);

    return false;
}

/**
 * Queues three requests behind this one and has their clients leave.
 *
 * @return array{queue_depth: int, active_connections: int, handled: int}|null
 *         the reading taken before any of it, or null when the staging did not
 *         get as far as it has to
 */
function queued_cancel_stage(TestCase $test): ?array
{
    // is_file first, and no @, for the reason test_breaker_abort_ignored gives.
    if (is_file(queued_cancel_marker())) {
        unlink(queued_cancel_marker());
    }

    $before = queued_cancel_metrics();
    $test->assertNotNull('/metrics exposes the queue, the connections and the worker-mode block', $before);
    if ($before === null) {
        return null;
    }

    $socks = [];
    for ($i = 0; $i < 3; $i++) {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        // No body: a request with nothing left to read is one whose connection
        // the server keeps reading while it waits, so the close is seen then and
        // not only once the request is answered.
        fwrite($sock, "GET /tests/breaker/queued_cancel_victim.php HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        $socks[] = $sock;
    }

    // The entry stays in the queue until a worker takes it, and the only worker
    // is running this request: three means all three are there and none has
    // been reached.
    $queued = queued_cancel_wait(static fn (array $m): bool => $m['queue_depth'] === 3);

    foreach ($socks as $sock) {
        fclose($sock);
    }

    $test->assertTrue('all three were waiting in the queue', $queued);
    if (!$queued) {
        return null;
    }

    // The connection count drops only after the connection's in-flight request
    // has been dropped with it, which is what marks that request cancelled. Back
    // where it started means the server has seen all three clients leave, while
    // their requests are still in the queue.
    $gone = queued_cancel_wait(
        static fn (array $m): bool => $m['active_connections'] === $before['active_connections']
            && $m['queue_depth'] === 3
    );
    $test->assertTrue('the server saw all three clients leave while their requests were queued', $gone);

    return $gone ? $before : null;
}
