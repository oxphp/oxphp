<?php

declare(strict_types=1);

// Shared between fibers/test_ini_is_the_requests_own and the test after it in
// the suite. PHP_WORKERS=1 in this profile, so a static set by one test request
// is there for the next.

final class OxphpIniProbeState
{
    public static ?string $basedirBaseline = null;

    /** Written by the task the first test leaves running, when it finishes. */
    public const TASK_DONE = '/tmp/oxphp-ini-last-out-task-done';
}
