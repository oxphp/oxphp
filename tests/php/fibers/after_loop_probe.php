<?php

declare(strict_types=1);

// Shared by the tests that read what the worker entry script's code after the
// oxphp_worker() call finds when the worker leaves its loop — see the end of
// tests/fixtures/worker/worker_entry.php. A test arms it before it makes the
// worker leave; the probe on the next suite line, served by the worker that
// replaced that one, reads what the code wrote.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpAfterLoop', false)) {
    final class OxphpAfterLoop
    {
        public static function armed(): string
        {
            return sys_get_temp_dir() . '/oxphp-entry-after-loop.armed';
        }

        public static function ran(): string
        {
            return sys_get_temp_dir() . '/oxphp-entry-after-loop.ran';
        }

        public static function arm(): void
        {
            self::clear();
            touch(self::armed());
        }

        /**
         * What the code after the loop wrote: the JSON of a callback's result,
         * 'null' if the engine refused to call it, null if the code never ran.
         *
         * Polled: the worker that replaces the one leaving can be serving this
         * request before the one leaving has run that code.
         */
        public static function wait(float $seconds = 5.0): ?string
        {
            $deadline = microtime(true) + $seconds;
            do {
                clearstatcache();
                if (is_file(self::ran())) {
                    return (string) file_get_contents(self::ran());
                }
                usleep(50_000);
            } while (microtime(true) < $deadline);

            return null;
        }

        /** is_file before unlink, not the silence operator: a registered error handler still runs under @. */
        public static function clear(): void
        {
            foreach ([self::armed(), self::ran()] as $file) {
                if (is_file($file)) {
                    unlink($file);
                }
            }
        }
    }
}
