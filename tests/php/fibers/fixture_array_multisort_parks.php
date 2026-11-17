<?php

declare(strict_types=1);

// Inner request for
// fibers/test_array_multisort_keeps_its_comparisons_while_another_request_waits.
// ?who= names it, ?park=N is how long the first comparison of its sort waits in
// oxphp_usleep(), in milliseconds. It publishes that it reached the wait as a
// latch, and when the wait began and ended.

require_once __DIR__ . '/multisort_probe.php';

$who = (string) ($_GET['who'] ?? '-');
$park = (int) ($_GET['park'] ?? 0);

$sorted = null;
$error = null;
try {
    $sorted = multisort_probe_sort(static function () use ($who, $park, &$sharedState): void {
        $sharedState['multisort_entered_' . $who] = true;
        $start = microtime(true);
        oxphp_usleep($park * 1000);
        $sharedState['multisort_wait_' . $who] = [$start, microtime(true)];
    });
} catch (\Throwable $e) {
    $error = get_class($e) . ': ' . $e->getMessage();
}

echo json_encode(['who' => $who, 'sorted' => $sorted, 'error' => $error]);
