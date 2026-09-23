<?php

declare(strict_types=1);

// Shared state for fibers/test_time_limit_is_not_restarted_by_a_park and the
// fixture it sends a request to. Pulled in with require_once, so in worker mode
// it outlives a single request and both of them read and write the same one: a
// request ended by its time limit cannot report through its response.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpTimeLimitProbe', false)) {
    final class OxphpTimeLimitProbe
    {
        /** How many times the request parked before it was ended, or finished. */
        public static int $parks = 0;

        /**
         * Bumped by the ticker request each time it runs. It can only run
         * while the request under test is parked, so a change across one of
         * that request's sleeps is what says the sleep parked.
         */
        public static int $ticks = 0;

        /** Raised when the request under test is over, to stop the ticker. */
        public static bool $over = false;

        /** Whether it ran all the way through. */
        public static bool $finished = false;

        /**
         * Whether its shutdown functions found the timeout bit up — the one
         * PHP raises when a request runs out of max_execution_time.
         */
        public static bool $timedOut = false;

        public static function reset(): void
        {
            self::$parks = 0;
            self::$finished = false;
            self::$timedOut = false;
            self::$ticks = 0;
            self::$over = false;
        }
    }
}
