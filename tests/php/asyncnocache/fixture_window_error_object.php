<?php

declare(strict_types=1);

// Required by a task of test_task_driver_window_error_does_not_reach_the_next_task,
// so that the async thread compiles the class itself. The task names no class
// of this request's script, which that thread does not have. `$mark` is the
// task's argument.
return new class ($mark) {
    public function __construct(private string $mark)
    {
    }

    public function __destruct()
    {
        file_put_contents($this->mark, 'ran');
        @trigger_error('oxphp-error-of-the-driver-window', E_USER_WARNING);
    }
};
