<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Baseline for the truncation flag: a profiled request that stays well
// under PROFILER_MAX_SPANS. Its run must come out with truncated=false —
// read back by profiler/test_truncated_index, which needs a negative case
// to tell "the flag is reported" from "the flag is always on".

$t = new TestCase('truncated_under_cap', 'profiler');

function _uc_leaf(int $n): int { return $n + 1; }
function _uc_outer(int $n): int { return _uc_leaf($n) + _uc_leaf($n); }

OxPHP\Profile\start();
$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += _uc_outer($i);
}
$t->assertTrue('profiling active', OxPHP\Profile\is_active());
$t->assertSame('work ran', $sum, 110);

$t->done();
