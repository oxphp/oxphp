<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\MemoryThreshold;

$t = new TestCase('decorator_memory_threshold', 'profiler');

// The `kb` argument is applied per attribute. `alloc_heavy` (1 KB
// threshold, allocates 256 KB) crosses its threshold; `alloc_light`
// (large threshold, allocates nothing) does not. This script is a
// smoke check on the execution path — the emitted span events are
// asserted in the Rust harness (tests/profiler_decorators_tests.rs).
#[MemoryThreshold(kb: 1)]
function alloc_heavy(): int
{
    $junk = str_repeat('x', 256 * 1024); // 256 KB
    return strlen($junk);
}

#[MemoryThreshold(kb: 8192)]
function alloc_light(): int
{
    return 1;
}

OxPHP\Profile\start();

$big = alloc_heavy();
$t->assertSame('memory-decorated heavy fn returns', $big, 256 * 1024);

$small = alloc_light();
$t->assertSame('memory-decorated light fn returns', $small, 1);

OxPHP\Profile\stop();
$t->done();
