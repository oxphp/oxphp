<?php

declare(strict_types=1);

// Pulled in by the tasks fibers/test_oom_at_the_end_of_a_task_spares_the_next_task
// spawns, on the async worker's thread: a class the request declares is not
// declared there. Conditionally, because that thread runs every task in the
// profile and keeps what one of them declares for the next.

if (!class_exists('OxphpTaskLateOomException', false)) {
    /**
     * Reported by the end of the task that threw it, which turns it into a
     * string — and that is where this one runs out of memory: in its own
     * __toString() ("handled"), or in what the report builds from the string it
     * returns ("pending"), which is C code with no point after it that looks for
     * an error not yet reported.
     */
    final class OxphpTaskLateOomException extends \RuntimeException
    {
        public function __construct(private string $marker, private string $case)
        {
            parent::__construct('task late exception');
        }

        public function __toString(): string
        {
            file_put_contents($this->marker, " tostring", FILE_APPEND);
            if ($this->case === 'pending') {
                // Under the memory_limit the task set on its own; the report's
                // copy of it is not.
                return str_repeat('x', 4 << 20);
            }
            $s = 'x';
            for (;;) {
                $s .= $s;
            }
        }
    }

    final class OxphpTaskLateOomHolder
    {
        public function __construct(private string $marker, private string $case)
        {
        }

        /** Leaves a mark that it ran, then throws. */
        public function __destruct()
        {
            file_put_contents($this->marker, 'destructor');
            throw new OxphpTaskLateOomException($this->marker, $this->case);
        }
    }
}
