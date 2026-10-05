<?php

declare(strict_types=1);

// Pulled in by the tasks fibers/test_async_task_teardown_throw_is_reported
// spawns, on the async worker's thread: a class the request declares is not
// declared there. Conditionally, because that thread runs every task in the
// profile and keeps what one of them declares for the next.

if (!class_exists('OxphpTaskTeardownThrower', false)) {
    final class OxphpTaskTeardownThrower
    {
        public function __construct(private string $marker, private string $tag)
        {
        }

        /** Leaves a mark that it ran, then throws. */
        public function __destruct()
        {
            file_put_contents($this->marker, 'ran');
            throw new \RuntimeException('task teardown throw: ' . $this->tag);
        }
    }
}
