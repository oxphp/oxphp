<?php

declare(strict_types=1);

// Loaded inside an async task: the object a task's child closure is bound to,
// which records when the closure lets go of it.
final class OrphanQueuedChildOwner
{
    public function __construct(private readonly string $released)
    {
    }

    public function child(string $ran): \Closure
    {
        return function () use ($ran): void {
            file_put_contents($ran, (string) microtime(true));
        };
    }

    public function __destruct()
    {
        file_put_contents($this->released, (string) microtime(true));
    }
}
