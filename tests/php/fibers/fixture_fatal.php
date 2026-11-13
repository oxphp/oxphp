<?php

declare(strict_types=1);

// The request tests/scripts/verify_worker_does_not_grow.sh measures a fatal by.
//
// ?fatal=1 raises a fatal from a distinctively named frame several calls deep.
// The fiber loop catches the bailout and carries on, but the bailout is a longjmp
// out of that frame: nothing rewinds the VM stack behind it or releases what its
// frames held unless the loop does.
//
// ?compile=1 fatals in a compile error instead, which abandons a compile partway:
// of eval'd code, or with ?compile=include of an included file.
//
// Otherwise the request ends normally, which makes it the control the fatals are
// measured against — and, sent between two fatals, it is what keeps the worker
// from being retired as one that fails every request.

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

if (isset($_GET['compile'])) {
    if ($_GET['compile'] === 'include') {
        include __DIR__ . '/fixture_compile_error.inc';
    }
    require_once __DIR__ . '/../engine_fatal.php';
    engine_fatal('OxphpProbeCompileFatal');
}

$names = array_map(
    static fn (array $f): string => (string) ($f['function'] ?? '?'),
    debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS)
);

header('Content-Type: text/plain');
echo 'FRAMES:', implode(',', $names), "\n";
echo 'DEPTH:', count($names), "\n";
