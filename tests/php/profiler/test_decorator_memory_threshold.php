<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\MemoryThreshold;

$t = new TestCase('decorator_memory_threshold', 'profiler');

// Register-time global default_kb = 64. The declared parameter is
// reserved for a follow-up that surfaces attribute args in
// DecoratorCallContext.
#[MemoryThreshold(kb: 1)]
function alloc_heavy(): int
{
    $junk = str_repeat('x', 256 * 1024); // 256 KB
    return strlen($junk);
}

#[MemoryThreshold(kb: 1)]
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
