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

// A session holding one of these cannot be decoded: the exception its wake-up
// throws has no frame to land in while the request starts up, so PHP bails out
// of php_request_startup.
final class WorkerTradThrowOnWake
{
    public function __wakeup(): void
    {
        throw new \RuntimeException('this session cannot be woken up');
    }
}
