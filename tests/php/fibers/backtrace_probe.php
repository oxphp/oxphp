<?php

declare(strict_types=1);

// Shared state for fibers/test_fatal_backtrace_stays_with_its_request and the
// requests it sends. Pulled in with require_once, so in worker mode it outlives
// a single request: the test, the request it parks and the request it sends in
// the window all read and write the same one.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpBacktraceProbe', false)) {
    final class OxphpBacktraceProbe
    {
        /** Up for as long as the parked request is inside its sleep. */
        public static bool $parked = false;

        /** The object the request sent in the window passes to its fatal. */
        public static ?\WeakReference $held = null;

        /**
         * Ends the request with a fatal raised two frames down, so that the
         * backtrace PHP 8.5 takes of it has a frame whose arguments are the
         * secret and the object.
         */
        public static function fatal(string $secret, object $held): never
        {
            trigger_error("backtrace probe fatal for $secret", E_USER_ERROR);
            exit(1);
        }

        /**
         * Whose fatal the backtrace error_get_last() reports belongs to: the
         * secret that fatal was called with, or 'none' when there is no
         * backtrace — which is always the answer before PHP 8.5, where
         * error_get_last() has no 'trace' key.
         *
         * @param array<string, mixed>|null $error
         */
        public static function traceSays(?array $error): string
        {
            if (!isset($error['trace'])) {
                return 'none';
            }
            foreach ($error['trace'] as $frame) {
                if (($frame['class'] ?? '') === self::class && ($frame['function'] ?? '') === 'fatal') {
                    return (string) ($frame['args'][0] ?? 'no-args');
                }
            }

            return 'unrecognised';
        }
    }
}
