<?php

declare(strict_types=1);

// Shared state for the tasks of fibers/test_async_task_fiber_stays_current_after_a_fatal_in_a_fiber.
//
// Pulled in with require_once from inside each task, so it is declared on the
// async worker thread and outlives the task that sets it: the task that fatals
// can return nothing, and records here instead what the next task reads back.

if (!class_exists('OxphpAsyncTaskFiberCurrentProbe', false)) {
    final class OxphpAsyncTaskFiberCurrentProbe
    {
        /** The fiber the fataling task ran in, by id. */
        public static ?int $fatalFiber = null;
    }
}
