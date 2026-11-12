<?php

declare(strict_types=1);

// Shared between the tests of what a worker runs while it recovers from a
// request's fatal and the inner requests they send. The worker is one thread,
// so what an inner request records here is what the outer request reads: a
// response cannot carry it, since the inner request is the one that fataled.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpRecoveryProbe', false)) {
    final class OxphpRecoveryProbe
    {
        /** What the inner request's decorator does in after(): 'sleep' or 'throw'. */
        public static string $case = '';

        /**
         * How far the inner request got: 'calling', then 'after-sleeping' and
         * 'after-slept', or 'after-throwing' and 'exception-destructing'.
         */
        public static ?string $stage = null;

        /** Whether a filter's onClose() ran while the worker gave a stream back. */
        public static bool $closeRan = false;

        /** Whether an exception handler the outer request installed was called. */
        public static bool $foreignHandlerRan = false;

        /** The fiber the inner request ran in, by id. */
        public static ?int $requestFiber = null;

        /**
         * Whether the session module refused to call a save handler it took for
         * one already running, and the fiber the error handler that warning
         * reached ran in, by id.
         */
        public static bool $warned = false;
        public static ?int $fiberAtWarning = null;

        public static function reset(): void
        {
            self::$case = '';
            self::$stage = null;
            self::$closeRan = false;
            self::$foreignHandlerRan = false;
            self::$requestFiber = null;
            self::$warned = false;
            self::$fiberAtWarning = null;
        }
    }
}

if (!function_exists('recovery_inner_send')) {
    /**
     * Sends an inner request to $path and returns the socket unread. Reading it
     * is what parks the caller and leaves the worker free to run the inner
     * request: this profile runs one worker.
     *
     * @return resource|false
     */
    function recovery_inner_send(string $path)
    {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return false;
        }
        fwrite($sock, "GET $path HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

        return $sock;
    }

    /** @param resource $sock */
    function recovery_inner_read($sock): string
    {
        stream_set_timeout($sock, 20);
        $resp = (string) stream_get_contents($sock);
        fclose($sock);

        return $resp;
    }
}
