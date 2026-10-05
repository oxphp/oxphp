<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('pool_thread_carries_no_execution_timer', 'asynctimeridle');

// An async pool thread must not carry an execution timer.
//
// The thread runs one php_request_startup() for its whole life, and that call
// arms the engine's per-thread execution timer with max_execution_time (3s
// here, see zz-asynctimeridle.ini). Nothing used to take it off, so it ran out
// once. This profile is for the firing that lands with the pool idle. Nothing is
// executing opcodes at that moment, so nothing is ended there; and on the build
// this catches, nothing is ended afterwards either, because admitting the next
// task onto an empty pool went through a reset that cleared EG(timed_out) —
// which is why the probe below runs to its end even on that build. The firing
// passed unnoticed, and what it left behind is the part no reset touches.
//
// What it leaves on the thread is php_on_timeout()'s PG(connection_status) |=
// PHP_CONNECTION_TIMEOUT, raised from the signal handler itself and so not
// waiting for any opcode to run. Nothing on the task path puts that field back
// — the reset between tasks does not touch it, and neither does entering a task
// fiber — so a later task reads 2 from connection_status() where a thread that
// was never left armed reads 0. connection_aborted() is a different bit of the
// same field and was never affected: php_on_timeout() ORs in only the timeout
// one.
//
// The idle stretch is measured, against the only clock that bounds where the
// arm fell. The server's own clock does not bound it: pool.start() returns
// without waiting for its threads, and the internal server comes up later, so
// /health answers while async-worker-0 is still inside its
// php_request_startup() — an uptime reading says nothing about the arm. What
// does bound it is that the thread cannot run a task before that startup
// returned, so a probe task's first opcode is at or after the arm. Holding the
// pool empty for longer than the limit past that timestamp therefore puts the
// mark behind, with nothing in flight for it to end.

$limit = 3; // max_execution_time of this profile

// The profile's limit bounds this request as well, counted from its own start,
// and the idle stretch below outlasts it on purpose. This thread's deadline is
// not what is under test, so it is lifted out of the way; the runner's own 15s
// cap still bounds the run.
set_time_limit(30);

// A probe task, dispatched for the timestamp of its first opcode alone. The
// arm is at or before that moment.
$probe = oxphp_async(function (): array {
    return ['at' => microtime(true)];
});

$probe_seen = null;
$probe_err = '';
try {
    $probe_seen = oxphp_async_await($probe, 10.0);
} catch (\Throwable $e) {
    $probe_err = get_class($e) . ': ' . $e->getMessage();
}

// Premise: the probe ran to its end, so its timestamp is an upper bound on the
// arm. A mark falling inside its handful of microseconds would have ended it
// instead — that is the sibling profile's case, not this one, and it fails here
// rather than being read as this one.
$t->assertSame('the probe task completed', $probe_err, '');
$t->assertTrue(
    'the probe task reported when it started',
    isset($probe_seen['at']) && is_float($probe_seen['at'])
);

// Nothing is in flight from here, and nothing is dispatched until the mark is
// behind: this is the idle stretch the firing has to land in.
usleep((int) (($limit + 1.0) * 1_000_000));
$idle_through = microtime(true);

$p = oxphp_async(function (): array {
    return [
        'status' => connection_status(),
        'aborted' => connection_aborted(),
    ];
});

$seen = null;
$err = '';
try {
    $seen = oxphp_async_await($p, 10.0);
} catch (\Throwable $e) {
    $err = get_class($e) . ': ' . $e->getMessage();
}

$t->assertSame('the task completed', $err, '');

// Premise: the pool really was left with nothing in flight from before the
// latest the arm can be until past the mark it set. Measured from the probe's
// timestamp, so a probe that did not report one fails here rather than being
// subtracted from as a zero — which would make this read as the whole of unix
// time and pass without having established anything.
$probe_at = $probe_seen['at'] ?? null;
$idle = is_float($probe_at) ? $idle_through - $probe_at : -1.0;
$t->assertGreaterThan(
    "the pool was idle past the {$limit}s mark (idle {$idle}s)",
    $idle,
    (float) $limit
);

$t->assertSame('the pool thread carries no timeout latch', $seen['status'] ?? null, 0);

// Not a second reading of the same thing: the field is a bitfield, and this
// pins which bit a firing would have left. It passes on a build that leaves the
// timer armed too — php_on_timeout() never raises the abort bit — so it is the
// assertion above that goes red there, and this one says why the field is 2
// rather than 1.
$t->assertSame('and the abort bit is not what a firing raises', $seen['aborted'] ?? null, 0);

$t->done();
