<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Drives a profiled request past PROFILER_MAX_SPANS (500 in this profile,
// see compose.profiler.yml) so the observer stops recording. The run must
// come out with truncated=true and exactly the cap's worth of spans;
// profiler/test_truncated_index reads both back off index.json.

$t = new TestCase('truncated_over_cap', 'profiler');

function _oc_leaf(int $n): int { return $n & 1; }

OxPHP\Profile\start();
$sum = 0;
for ($i = 0; $i < 2000; $i++) {
    $sum += _oc_leaf($i);
}
$t->assertSame('work ran', $sum, 1000);

$t->done();
