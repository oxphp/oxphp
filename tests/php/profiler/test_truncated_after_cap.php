<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Runs straight after test_truncated_over_cap, on the same PHP worker
// (this profile is pinned to one), and stays far under the cap. Its run must
// come out with truncated=false: the flag the previous request raised lives
// in thread-local bridge state, and a build that failed to clear it between
// requests would report every later run on that worker as truncated for the
// rest of the process.

$t = new TestCase('truncated_after_cap', 'profiler');

function _ac_leaf(int $n): int { return $n + 1; }

OxPHP\Profile\start();
$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += _ac_leaf($i);
}
$t->assertSame('work ran', $sum, 55);

$t->done();
