<?php

declare(strict_types=1);

// Shared state for the fibers/test_stream_backpressure_* tests and the fixture
// they send requests to. Pulled in with require_once, so in worker mode it
// outlives a single request and both of them read and write the same one: the
// fixture is a stream whose client is not reading, and it cannot report through
// its response.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpStreamBackpressureProbe', false)) {
    final class OxphpStreamBackpressureProbe
    {
        /** How far the fixture got: null, 'flushing' (inside a flush), 'flushed',
         *  'fatal' (about to run out of memory_limit), 'done'. */
        public static ?string $stage = null;

        /** How many of its flushes have returned. */
        public static int $flushes = 0;

        /** Raised by its shutdown function, which runs however it ended. */
        public static bool $ended = false;

        /** What connection_status() said in that shutdown function. */
        public static int $connectionStatus = 0;

        /** The message of error_get_last() there, or null. */
        public static ?string $lastError = null;

        /** microtime(true) when it started and when its shutdown function ran. */
        public static float $startedAt = 0.0;
        public static float $endedAt = 0.0;

        /** Set by a test for the fixture to act on after its next flush: 'fatal'. */
        public static ?string $command = null;

        /** How many flushes it made after it saw its client gone (?after=). */
        public static int $flushesAfterGone = 0;

        /** Its resident memory, in bytes, before and after those flushes. */
        public static int $rssBefore = 0;
        public static int $rssAfter = 0;

        /** What the fixture's ?alloc= request saw as it allocated: the stream's
         *  stage and whether it had ended, and the error it ended with, if any. */
        public static ?string $allocStage = null;
        public static ?bool $allocEnded = null;
        public static ?string $allocError = null;

        public static function reset(): void
        {
            self::$stage = null;
            self::$flushes = 0;
            self::$ended = false;
            self::$connectionStatus = 0;
            self::$lastError = null;
            self::$startedAt = 0.0;
            self::$endedAt = 0.0;
            self::$command = null;
            self::$flushesAfterGone = 0;
            self::$rssBefore = 0;
            self::$rssAfter = 0;
            self::$allocStage = null;
            self::$allocEnded = null;
            self::$allocError = null;
        }
    }
}

if (!function_exists('stream_backpressure_chunk')) {
    /** This process's resident memory, in bytes. */
    function stream_backpressure_rss(): int
    {
        $status = (string) file_get_contents('/proc/self/status');

        return preg_match('/^VmRSS:\s+(\d+) kB/m', $status, $m) === 1 ? (int) $m[1] * 1024 : 0;
    }

    /** The $i-th chunk the fixture sends: $size bytes, one letter repeated. */
    function stream_backpressure_chunk(int $i, int $size): string
    {
        return str_repeat(chr(ord('a') + $i % 26), $size);
    }

    /**
     * Polls until $until returns true, for at most $seconds.
     *
     * usleep is hooked in this profile, so every wait parks this request and
     * leaves the worker free to run the fixture.
     */
    function stream_backpressure_wait(callable $until, float $seconds = 3.0): bool
    {
        $deadline = microtime(true) + $seconds;
        do {
            if ($until()) {
                return true;
            }
            usleep(10_000);
        } while (microtime(true) < $deadline);

        return false;
    }

    /**
     * Opens a connection to the fixture and sends it a request, reading nothing.
     *
     * HTTP/1.0, so the body comes back as it is written, unframed, and ends
     * when the connection does.
     *
     * @return resource|false
     */
    function stream_backpressure_open(string $query)
    {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return false;
        }
        fwrite($sock, "GET /tests/fibers/fixture_stream_backpressure.php?$query HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

        return $sock;
    }

    /**
     * Waits until the fixture is stuck in a flush: inside one, and no flush has
     * returned for $quietSeconds. Read again by the caller wherever it needs the
     * premise to hold, since it says nothing about any later moment.
     */
    function stream_backpressure_stalled(float $quietSeconds = 0.2, float $seconds = 5.0): bool
    {
        return stream_backpressure_wait(static function () use ($quietSeconds): bool {
            if (OxphpStreamBackpressureProbe::$stage !== 'flushing') {
                return false;
            }
            $flushes = OxphpStreamBackpressureProbe::$flushes;
            usleep((int) ($quietSeconds * 1_000_000));

            return OxphpStreamBackpressureProbe::$stage === 'flushing'
                && OxphpStreamBackpressureProbe::$flushes === $flushes;
        }, $seconds);
    }
}
