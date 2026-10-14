<?php

declare(strict_types=1);

// Preloaded, so the class exists while a request starts up: the session
// session.auto_start decodes there wakes its object up before any script runs,
// and before OxPHP's own module has started the request.
final class WorkerTradWakeProbe
{
    public static ?object $worker = null;

    public function __wakeup(): void
    {
        self::$worker = \OxPHP\Server\Worker::current();
    }
}
