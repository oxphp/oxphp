<?php

declare(strict_types=1);

// Inner self-request for fibers/test_vm_stack_rewound_after_fatal. Two of these
// run inside one suspended window, so the second lands on the fiber the first
// returned to the free list.
//
// ?fatal=1 raises a fatal from a distinctively named frame several calls deep.
// The fiber loop catches the bailout and carries on, but the bailout is a longjmp
// out of that frame: nothing rewinds the VM stack behind it unless the loop does.
//
// Otherwise: report the call chain this request is standing on. If the previous
// request's abandoned frames were left on the stack, they are what this one's
// frames chain onto, and they show up here.
//
// Must not suspend — no await, no sleep, no socket read — or the second request
// gets a fresh fiber and the test covers nothing.

// Declared conditionally on purpose: the worker keeps this process across
// requests, and a second unconditional declaration of the same name would fail
// the request for a reason that has nothing to do with what is under test.
if (!function_exists('oxphp_probe_frame_c')) {
    function oxphp_probe_frame_c(): void
    {
        trigger_error('probe fatal', E_USER_ERROR);
    }

    function oxphp_probe_frame_b(): void
    {
        oxphp_probe_frame_c();
    }

    function oxphp_probe_frame_a(): void
    {
        oxphp_probe_frame_b();
    }
}

if (isset($_GET['fatal'])) {
    oxphp_probe_frame_a();
    exit; // not reached
}

$names = array_map(
    static fn (array $f): string => (string) ($f['function'] ?? '?'),
    debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS)
);

header('Content-Type: text/plain');
echo 'FRAMES:', implode(',', $names), "\n";
echo 'DEPTH:', count($names), "\n";
