<?php

declare(strict_types=1);

// Preloaded, so the class exists while a request starts up: the session
// session.auto_start decodes there wakes its object up before any script runs.
final class PoolStartupWake
{
    public static bool $woken = false;
    public static int $frames = 0;
    public static ?string $threw = null;
    public static string $message = '';

    public function __wakeup(): void
    {
        self::$woken = true;
        self::$frames = count(debug_backtrace());
        try {
            \OxPHP\Shared\Registry::pool('poolstartup', static fn (): \OxPHP\Shared\Pool => new \OxPHP\Shared\Pool(
                factory: static fn (): \ArrayObject => new \ArrayObject(),
                destroy: null,
                maxSize: 1,
            ));
        } catch (\Throwable $e) {
            self::$threw = $e::class;
            self::$message = $e->getMessage();
        }
    }
}
