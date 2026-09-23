<?php

declare(strict_types=1);

// Shared between the session-write tests and their inner requests: the worker is
// one thread, so what an inner request's save handler records here is what the
// outer request, or another inner request, reads.

final class OxphpSessionWriteProbe
{
    public static ?string $written = null;

    /** True while a save handler's write is under way. */
    public static bool $writing = false;

    public static function reset(): void
    {
        self::$written = null;
        self::$writing = false;
    }
}
