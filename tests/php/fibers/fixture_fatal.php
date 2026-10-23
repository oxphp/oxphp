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
        oxphp_probe_fatal();
        oxphp_probe_fatal();
    }

    // The fatal, raised with nothing being compiled. Not engine_fatal(): a
    // compile error strands what the compiler was building, which a worker then
    // keeps, and what a fatal leaves behind on a worker is what this fixture is
    // measured by (tests/scripts/verify_worker_does_not_grow.sh). A class
    // declared at run time fatals with E_COMPILE_ERROR all the same once its
    // name is taken, and no error handler is handed that either. Called twice
    // so that it fatals on a worker no earlier request has declared it on.
    function oxphp_probe_fatal(): void
    {
        class OxphpProbeFatal
        {
        }
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
