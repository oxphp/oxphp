<?php

declare(strict_types=1);

// Objects that leave a file behind when their destructor runs, for the tests
// that a fatal in one request does not take the destructors of what another
// request, task or the worker itself holds. A file rather than a static: a
// static set by a destructor that never ran reads the same as one that was
// never reset, and the trigger and its probe can run on different workers.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpDestructProbe', false)) {
    final class OxphpDestructProbe
    {
        private const MARK = '/tmp/oxphp-destruct-probe-';

        /** Held by the worker rather than by any request. */
        public static ?self $kept = null;

        /** Set to the object itself, which only the cycle collector then frees. */
        public ?self $cycle = null;

        public function __construct(private readonly string $name)
        {
        }

        public function __destruct()
        {
            self::mark($this->name);
        }

        /** Leaves the file for $name, which destructed() then reads. */
        public static function mark(string $name): void
        {
            file_put_contents(self::MARK . $name, '1');
        }

        /** @param list<string> $names */
        public static function reset(array $names): void
        {
            foreach ($names as $name) {
                if (is_file(self::MARK . $name)) {
                    unlink(self::MARK . $name);
                }
            }
        }

        public static function destructed(string $name): bool
        {
            clearstatcache();

            return is_file(self::MARK . $name);
        }
    }
}

if (!function_exists('oxphp_destruct_probe_fatal')) {
    // A fatal the engine bails out of: a class declared at run time fails with
    // E_COMPILE_ERROR once its name is taken. Called twice so that it fails on a
    // worker no earlier request has declared it on. Not a timeout and not a
    // memory error, which reach the engine's report through other paths.
    function oxphp_destruct_probe_fatal(): void
    {
        oxphp_destruct_probe_declare();
        oxphp_destruct_probe_declare();
    }

    function oxphp_destruct_probe_declare(): void
    {
        class OxphpDestructProbeFatal
        {
        }
    }
}
