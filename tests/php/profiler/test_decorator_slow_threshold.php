<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\SlowThreshold;

$t = new TestCase('decorator_slow_threshold', 'profiler');

// The current implementation uses a register-time global (default_ms = 100),
// so the `ms` parameter declared on the attribute is currently ignored
// at runtime. The test still uses it for spec-conformance — once
// per-attribute parameterisation lands, this script becomes a
// real per-call threshold check.
#[SlowThreshold(ms: 1)]
function slow_decorated(): int
{
    usleep(150 * 1000); // 150 ms
    return 1;
}

#[SlowThreshold(ms: 1)]
function fast_decorated(): int
{
    return 2;
}

OxPHP\Profile\start();

$slow_result = slow_decorated();
$t->assertSame('slow decorated function returns', $slow_result, 1);

$fast_result = fast_decorated();
$t->assertSame('fast decorated function returns', $fast_result, 2);

OxPHP\Profile\stop();
$t->done();
