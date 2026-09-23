<?php

declare(strict_types=1);

// Inner request for fibers/test_cancel_runs_userland_cleanup.
//
// Calls the guarded render with the key its test gave it, and does nothing
// else. The key comes from the request rather than being fixed here because
// the mark it leaves is on the worker, not on the request: a run that leaves
// one behind would otherwise be indistinguishable from the next run finding
// its own, and the test would pass on the wreckage of the run before it.

require_once __DIR__ . '/cancel_finally_guard.php';

oxphp_cancel_guarded_render((string) ($_GET['key'] ?? 'fixed'));
