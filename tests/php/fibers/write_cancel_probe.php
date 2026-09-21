<?php

declare(strict_types=1);

// Shared state for fibers/test_write_cancel_releases_frames and the fixture it
// sends requests to.
//
// Pulled in with require_once, so in worker mode it outlives a single request
// and is shared by both of them: the fixture writes what happened to it here,
// the test reads it here. A response cannot carry it, since the point of the
// fixture is that its client has gone.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpWriteCancelProbe', false)) {
    final class OxphpWriteCancelProbe
    {
        /**
         * How far the fixture got: null, 'parked', 'before-write', 'after-write';
         * with ?in=destructor, then 'destructing' and 'destructed'.
         */
        public static ?string $stage = null;

        /** What the fixture's frame was holding, weakly. */
        public static ?\WeakReference $weak = null;

        /** @var array{stage: ?string, freed: bool, last_error: ?string}|null */
        public static ?array $report = null;

        /** How far the fixture had got when the object it held was destroyed. */
        public static ?string $stageAtDestruct = null;

        /**
         * What connection_aborted() answered the fixture after the write whose
         * client had gone — the question a script has to be able to ask, now
         * that the answer is what it stops on instead of being stopped.
         */
        public static ?int $abortedAfterWrite = null;

        public static function reset(): void
        {
            self::$stage = null;
            self::$weak = null;
            self::$report = null;
            self::$stageAtDestruct = null;
            self::$abortedAfterWrite = null;
        }

        /** Whether what the fixture's frame was holding is gone. */
        public static function freed(): bool
        {
            return self::$weak !== null && self::$weak->get() === null;
        }
    }
}

if (!function_exists('write_cancel_active_connections')) {
    /** The server's count of open client connections, or null when /metrics is unreachable. */
    function write_cancel_active_connections(): ?int
    {
        $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
        $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
        if (!is_string($body) || !preg_match('/^oxphp_active_connections (\d+)$/m', $body, $m)) {
            return null;
        }

        return (int) $m[1];
    }

    /**
     * Polls until $until returns true, for at most $seconds.
     *
     * usleep is hooked in this profile, so every wait parks this request and
     * leaves the worker free to run the fixture.
     */
    function write_cancel_wait(callable $until, float $seconds = 3.0): bool
    {
        $deadline = microtime(true) + $seconds;
        do {
            if ($until()) {
                return true;
            }
            usleep(20_000);
        } while (microtime(true) < $deadline);

        return false;
    }

    /**
     * Sends the fixture a request, takes its client away while it is parked, and
     * waits until the server has seen the client go. Returns whether all of that
     * happened; each step is asserted as it is taken.
     *
     * $fixture names the inner request's script. Several tests stage a client
     * that leaves mid-request and differ only in what the request was doing at
     * the time, so the staging is written once here and the fixture is theirs.
     */
    function write_cancel_stage(
        TestCase $t,
        string $label,
        string $query,
        string $fixture = 'fixture_write_cancel_release.php'
    ): bool {
        OxphpWriteCancelProbe::reset();

        $before = write_cancel_active_connections();
        $t->assertNotNull("$label: /metrics reports the open client connections", $before);

        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        $t->assertTrue("$label: inner request socket connected", $sock !== false);
        if ($before === null || $sock === false) {
            return false;
        }

        // No body, so the server keeps reading the connection while the request
        // runs and sees the close as soon as it happens.
        fwrite($sock, "GET /tests/fibers/$fixture$query HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

        // Closed only once the inner request is running: a request whose client
        // leaves while it is still queued is dropped before it starts, and would
        // exercise nothing here.
        $parked = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$stage === 'parked');
        fclose($sock);
        $t->assertTrue("$label: the inner request was parked when its client left", $parked);

        // The count drops once the connection's request has been dropped with it,
        // which is what marks that request cancelled. Still parked at that reading
        // means the cancellation reached it during the park, not after it had
        // moved on.
        $gone = write_cancel_wait(
            static fn (): bool => write_cancel_active_connections() === $before
                && OxphpWriteCancelProbe::$stage === 'parked'
        );
        $t->assertTrue("$label: the server saw the client leave while the inner request was parked", $gone);

        // Marking the request cancelled also raises the worker's interrupt flag,
        // and the flag is acted on by whichever request runs PHP next. The reading
        // above already returned from calls in this request after it was raised;
        // this is one more, so the flag is spent here rather than in the inner
        // request when it resumes — there it would end the request with an error
        // before it reached the write.
        microtime(true);

        return $parked && $gone;
    }
}
