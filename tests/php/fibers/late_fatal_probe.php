<?php

declare(strict_types=1);

// Shared by the tests that a fatal inside serialize() raised past the request's
// handler — in a shutdown function, in the write of the session, in code the
// worker runs between requests — does not leave the next serialize() on that
// stack the table of the call it ended.
//
// Pulled in with require_once for the reason fiber_park_registry.php gives.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/thread_cell_probe.php';

if (!class_exists('OxphpLateFatalBadSleep', false)) {
    /**
     * Names a property it does not have. serialize() warns about it after
     * __sleep() has returned, with the call still open, and an error handler
     * that turns the warning into a fatal ends the call there.
     */
    final class OxphpLateFatalBadSleep
    {
        public int $v = 1;

        /** @return list<string> */
        public function __sleep(): array
        {
            return ['missing'];
        }
    }

    /** What the probes record where nothing else can carry it out. */
    final class OxphpLateFatalProbe
    {
        /** @var array{same: bool, filled: bool}|null */
        public static ?array $close = null;
    }

    /** Fails inside serialize() when it is released. */
    final class OxphpLateFatalOnRelease
    {
        public function __construct(private string $marker)
        {
        }

        public function __destruct()
        {
            file_put_contents($this->marker, '1');
            oxphp_late_fatal_inside_serialize('OxphpLateFatalOnReleaseBroken');
        }
    }

    /** Records what serialize() and unserialize() do when it is released. */
    final class OxphpLateProbeOnRelease
    {
        public function __construct(private string $file)
        {
        }

        public function __destruct()
        {
            file_put_contents($this->file, json_encode(oxphp_late_probe()));
        }
    }
}

if (!function_exists('oxphp_late_probe')) {
    /**
     * thread_cell_unserialize_probe() with its warnings kept from the handler
     * installed on the worker, which may be a test's that throws on any: a
     * back-reference resolved against a table a fatal left can name no value
     * at all, and unserialize() then warns as it returns false.
     *
     * @return array{same: bool, filled: bool}
     */
    function oxphp_late_probe(): array
    {
        set_error_handler(static fn (): bool => true);
        try {
            return thread_cell_unserialize_probe();
        } finally {
            restore_error_handler();
        }
    }
}

if (!function_exists('oxphp_late_fatal_inside_serialize')) {
    /**
     * Ends a serialize() call with a fatal error while the call is open. The
     * handler takes itself off first, because a handler set on a worker stays
     * there for the requests after this one.
     */
    function oxphp_late_fatal_inside_serialize(string $tag): never
    {
        set_error_handler(static function () use ($tag): bool {
            restore_error_handler();
            engine_fatal($tag);
        });
        serialize(new OxphpLateFatalBadSleep());

        throw new \LogicException('serialize() did not warn about the missing property');
    }
}
