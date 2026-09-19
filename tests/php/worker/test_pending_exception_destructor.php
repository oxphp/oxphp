<?php

declare(strict_types=1);

// Three guarantees about how a worker discards an uncaught exception, one pair
// of phases each. Every pair talks through a file rather than worker-scope
// state, so they hold under any pool size — the marker lives on the container
// filesystem, whichever worker answers.
//
// Pair 1 — `throw` / `check`: the exception's class declares a __destruct, and
// that destructor runs. For the shape this covers — an exception carrying a file
// lock, a temp file or an open handle it gives back on the way out — the
// destructor is the only thing that gives the resource back. Asserting that
// rather than "the request answered 500" is the point: the 500 comes back either
// way, so the status says nothing about whether the object was discarded or
// destroyed half-way through. The engine refuses to destruct the object that is
// still the pending exception and makes the refusal a core error, so a discard
// that releases before it clears the slot never reaches the destructor at all —
// the object is flagged as destructed on the way into that error and stays
// flagged, which is why the marker's absence is a proof and not a race.
//
// Pair 2 — `dtorthrow` / `dtorcheck`: once the destructor does run it is user
// code, and user code can throw. The discard has to keep draining until the slot
// is empty, because the next thing the request does is call its shutdown
// functions — and zend_call_function returns without calling anything while an
// exception is pending, so a single clear would let one throwing destructor skip
// every shutdown function the application registered, in order, in silence.
//
// Pair 3 — `dtorloop` / `dtorloopcheck`: the draining needs a ceiling as much as
// it needs to happen. Two classes whose destructors throw each other give the
// drain a fresh exception every turn while freeing the previous one, so memory
// stays flat and no limit ever arrives, and the request holds its worker thread
// until something outside it intervenes. Measured on a build with the ceiling
// removed: with the execution deadline in force the spin ends at
// max_execution_time with a 504, and with the deadline lifted — which is what a
// streaming script is told to do, and what this phase does — it ends only when
// the client stops waiting. So what the check asserts is how long the request
// took, not that it finished: it finishes on both builds, microseconds after
// the throw on one and a client's patience later on the other.

$markerDestruct = sys_get_temp_dir() . '/oxphp_worker_pending_exception_destructor';
$markerShutdown = sys_get_temp_dir() . '/oxphp_worker_dtor_throw_shutdown';
$markerLoop = sys_get_temp_dir() . '/oxphp_worker_dtor_loop_shutdown';

// Declared conditionally because in worker mode this file is included afresh on
// every request that names it, including the check phases below.
if (!class_exists('PendingExceptionDestructorProbe', false)) {
    class PendingExceptionDestructorProbe extends RuntimeException
    {
        public function __construct(private string $marker)
        {
            parent::__construct('probe');
        }

        public function __destruct()
        {
            file_put_contents($this->marker, 'destructed');
        }
    }

    class ThrowingDestructorProbe extends RuntimeException
    {
        public function __construct()
        {
            parent::__construct('probe with a throwing destructor');
        }

        public function __destruct()
        {
            throw new LogicException('thrown out of __destruct');
        }
    }

    // Each one's destructor throws the other, so a discard that drains without a
    // ceiling is handed a new exception every turn and never reaches an empty
    // slot. Deliberately not self-referential: a class throwing its own type
    // would do as well, but naming two makes it obvious from the trace which
    // turn of the loop a diagnostic came from.
    class PingDestructorProbe extends RuntimeException
    {
        public function __destruct()
        {
            throw new PongDestructorProbe('pong');
        }
    }

    class PongDestructorProbe extends RuntimeException
    {
        public function __destruct()
        {
            throw new PingDestructorProbe('ping');
        }
    }
}

$phase = $_GET['phase'] ?? 'throw';

if ($phase === 'throw') {
    if (is_file($markerDestruct)) {
        unlink($markerDestruct);
    }

    throw new PendingExceptionDestructorProbe($markerDestruct);
}

if ($phase === 'dtorthrow') {
    if (is_file($markerShutdown)) {
        unlink($markerShutdown);
    }

    // Registered before the throw, so it is on the list the request is supposed
    // to run on its way out no matter how the handler ended.
    register_shutdown_function(static function () use ($markerShutdown): void {
        file_put_contents($markerShutdown, 'ran');
    });

    throw new ThrowingDestructorProbe();
}

if ($phase === 'dtorloop') {
    if (is_file($markerLoop)) {
        unlink($markerLoop);
    }

    // The one thing that would otherwise stop an unbounded drain, removed the
    // way a streaming endpoint is told to remove it. Leave it in force and the
    // engine's deadline ends the spin instead — measured at 30 seconds, with a
    // 504 — which would make this pair assert that something outside the
    // discard eventually kills the request, rather than that the discard ends.
    set_time_limit(0);

    $startedAt = microtime(true);

    // What the shutdown function records is how long the request took, not
    // that it happened. Both builds get here in the end: with the ceiling the
    // drain stops by itself in microseconds, without it the request runs until
    // the client gives up and the cancellation unwinds it — the shutdown
    // function then runs too, and a marker that only said 'ran' would be
    // written either way.
    register_shutdown_function(static function () use ($markerLoop, $startedAt): void {
        file_put_contents($markerLoop, sprintf('%.3f', microtime(true) - $startedAt));
    });

    throw new PingDestructorProbe('ping');
}

require_once __DIR__ . '/../test_helper.php';

// Guarded rather than suppressed: TestCase turns every warning into an
// ErrorException, and `@` does not stop a custom error handler from being
// called, so reading a path that is not there would fatal the test.
$read = static function (string $path): string {
    if (!is_file($path)) {
        return '';
    }
    $written = (string) file_get_contents($path);
    unlink($path);

    return $written;
};

if ($phase === 'dtorloopcheck') {
    $t = new TestCase('pending_exception_dtor_loop_bounded', 'worker');
    $elapsed = $read($markerLoop);
    $t->assertNotEmpty(
        "the request that threw mutually throwing destructors reached its shutdown functions ({$markerLoop})",
        $elapsed
    );
    // Seconds, and generous: the bounded discard takes microseconds, while a
    // drain with no ceiling holds the request for as long as the client waits
    // — the runner gives one 15 seconds.
    $t->assertLessThan(
        'the discard ended by itself rather than when something killed the request',
        (float) $elapsed,
        2.0
    );
    $t->done();

    return;
}

if ($phase === 'dtorcheck') {
    $t = new TestCase('pending_exception_dtor_throw_shutdown', 'worker');
    $t->assertSame(
        "a shutdown function still ran after a throwing __destruct ({$markerShutdown})",
        $read($markerShutdown),
        'ran'
    );
    $t->done();

    return;
}

$t = new TestCase('pending_exception_destructor', 'worker');
$t->assertSame(
    "the previous request's exception ran its __destruct ({$markerDestruct})",
    $read($markerDestruct),
    'destructed'
);
$t->done();
