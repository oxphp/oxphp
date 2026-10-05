<?php

declare(strict_types=1);

// Shared state for the workertimer tests and the requests they send. Pulled in
// with require_once, so in worker mode it outlives a single request and a test
// and the inner requests it starts read and write the same one: a request ended
// by its time limit cannot report through its response.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpWorkerTimerProbe', false)) {
    final class OxphpWorkerTimerProbe
    {
        /**
         * Raised by a test for exactly as long as it waits on the requests it
         * sent. A request that finds it raised when it starts was taken while
         * the test was in flight on the same worker — by the event loop, not
         * by the worker picking up a request with nothing else running.
         */
        public static bool $outerInFlight = false;

        /**
         * One entry per request under test, keyed by the name it registers.
         *
         * @var array<string, array{beside: bool, started: int, ended: int, finished: bool, timedOut: bool, woke: bool, message: string}>
         */
        public static array $runs = [];

        /**
         * Seconds a shutdown function of a request that ran out of time spent
         * parked, recorded after the park. Null until that shutdown function
         * gets past it.
         */
        public static ?float $cleanupSlept = null;

        /**
         * A connection a test keeps open past its own end, so that the request
         * it was sent on is not ended by its client going away.
         *
         * @var resource|null
         */
        public static $heldOpen = null;

        public static function reset(): void
        {
            self::$outerInFlight = false;
            self::$runs = [];
            self::$cleanupSlept = null;
            self::$heldOpen = null;
        }

        /**
         * Record that the calling request has started, and have its shutdown
         * functions record how it ended: when, whether PHP had raised the
         * timeout bit — the one it raises when a request runs out of
         * max_execution_time — and the last error, which for a request ended
         * by its limit names the limit.
         */
        public static function start(string $name): void
        {
            self::$runs[$name] = [
                'beside' => self::$outerInFlight,
                'started' => hrtime(true),
                'ended' => 0,
                'finished' => false,
                'timedOut' => false,
                'woke' => false,
                'message' => '',
            ];

            register_shutdown_function(static function () use ($name): void {
                self::$runs[$name]['ended'] = hrtime(true);
                self::$runs[$name]['timedOut'] = (connection_status() & CONNECTION_TIMEOUT) !== 0;
                self::$runs[$name]['message'] = error_get_last()['message'] ?? '';
            });
        }

        /** Seconds from the request's start to its shutdown functions. */
        public static function lived(string $name): float
        {
            $run = self::$runs[$name];

            return ($run['ended'] - $run['started']) / 1e9;
        }

        /**
         * Busy for about $seconds in slices of fifty milliseconds, parking after
         * each one. Busy so the time is spent running PHP, where a time limit is
         * able to end it; parking so the worker gets to run the other requests
         * between the slices, and the request comes back to its limit through a
         * resume every time.
         */
        public static function burn(float $seconds): void
        {
            $stop = microtime(true) + $seconds;
            while (microtime(true) < $stop) {
                $until = microtime(true) + 0.05;
                while (microtime(true) < $until) {
                    // burn
                }
                // Hooked in this profile: parks this request and hands the
                // worker back.
                usleep(1000);
            }
        }

        /**
         * Open a connection to this server and send it a request for $path.
         *
         * @return resource
         */
        public static function send(string $path)
        {
            $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
            if ($sock === false) {
                throw new \RuntimeException("connect failed: $errstr ($errno)");
            }
            fwrite($sock, "GET $path HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

            return $sock;
        }

        /**
         * Read a response to its end. The read parks the caller, which is what
         * lets the worker take the request it is waiting for.
         *
         * @param resource $sock
         * @return array{0: int, 1: string} status code (0 when none came) and body
         */
        public static function receive($sock, float $timeout): array
        {
            stream_set_timeout($sock, (int) ceil($timeout));
            $raw = (string) stream_get_contents($sock);
            fclose($sock);

            $status = preg_match('#^HTTP/\d\.\d (\d{3})#', $raw, $m) ? (int) $m[1] : 0;
            $split = strpos($raw, "\r\n\r\n");

            return [$status, $split === false ? '' : substr($raw, $split + 4)];
        }
    }
}
