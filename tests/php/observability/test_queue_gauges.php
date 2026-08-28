<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Behavioural contract: the three queue gauges are read off the live
// admission gate, not counted somewhere and hoped to match. Only an
// end-to-end scrape proves the executor installed the probe at all —
// unit tests install their own and would pass with the wiring missing.
//
// The reading this pins is the healthy one: a server answering this very
// request has admission slots free. The state these gauges exist for is
// its opposite — zero slots available with nothing queued, which is the
// pool refusing every request while its workers sit idle.

$test = new TestCase('queue_gauges', 'observability');

$metrics = @file_get_contents('http://127.0.0.1:9090/metrics');
$test->assertTrue('metrics endpoint reachable', is_string($metrics));

if (!is_string($metrics)) {
    $test->done();
    return;
}

$read = static function (string $name) use ($metrics): ?int {
    if (preg_match('/^' . preg_quote($name, '/') . ' (\d+)$/m', $metrics, $m) !== 1) {
        return null;
    }
    return (int) $m[1];
};

$depth = $read('oxphp_queue_depth');
$capacity = $read('oxphp_queue_capacity');
$available = $read('oxphp_admission_slots_available');

$test->assertTrue('oxphp_queue_depth exposed', $depth !== null);
$test->assertTrue('oxphp_queue_capacity exposed', $capacity !== null);
$test->assertTrue('oxphp_admission_slots_available exposed', $available !== null);

if ($depth === null || $capacity === null || $available === null) {
    $test->done();
    return;
}

$test->assertTrue('queue capacity is positive (got ' . $capacity . ')', $capacity > 0);
$test->assertTrue(
    'queue depth within capacity (' . $depth . ' <= ' . $capacity . ')',
    $depth <= $capacity
);
$test->assertTrue(
    'slots available within capacity (' . $available . ' <= ' . $capacity . ')',
    $available <= $capacity
);
// A server that answered this request has room for the next one. Zero
// here on an idle server is the fault these gauges were added for.
$test->assertTrue(
    'admission has slots free while serving (got ' . $available . ')',
    $available > 0
);

$test->done();
