<?php
/**
 * Cross-thread fcc invocation probe.
 *
 * Two modes via the `op` query parameter:
 *
 *   op=capture → captures a closure's fcc under the current
 *                worker's thread id and returns that tid.
 *   op=invoke  → invokes the captured callable and reports both
 *                the capture tid and the current invoker tid.
 *   op=reset   → drops the captured callable (run on the original
 *                capturing thread to avoid cross-thread teardown).
 *
 * Paired with `tests/run_pool_spike.sh` which hammers a multi-worker
 * OxPHP instance and asserts at least one cross-thread invocation
 * succeeds — the decision gate for the Pool architecture.
 */
header('Content-Type: text/plain');

$op = $_GET['op'] ?? 'help';

switch ($op) {
    case 'capture': {
        $factory = function () {
            // Closure captures no use-vars and no `$this` so the op_array
            // it compiles into is as minimal as possible — any crash
            // we see reflects fcc/op_array cross-thread visibility,
            // not leaked-symbol-table hazards.
            return 'spike-value-42';
        };
        $tid = oxphp_pool_spike_capture($factory);
        printf("captured tid=%d\n", $tid);
        exit;
    }

    case 'invoke': {
        try {
            $info = oxphp_pool_spike_invoke();
        } catch (\OxPHP\Shared\UninitializedException $e) {
            echo "FAIL: capture-first: " . $e->getMessage() . "\n";
            exit;
        }
        printf(
            "invoked captured_tid=%d current_tid=%d cross_thread=%s result=%s\n",
            $info['captured_tid'],
            $info['current_tid'],
            $info['cross_thread'] ? 'yes' : 'no',
            is_string($info['result']) ? $info['result'] : var_export($info['result'], true)
        );
        exit;
    }

    case 'reset':
        oxphp_pool_spike_reset();
        echo "reset OK\n";
        exit;

    default:
        echo "usage: ?op=capture | ?op=invoke | ?op=reset\n";
        exit;
}
