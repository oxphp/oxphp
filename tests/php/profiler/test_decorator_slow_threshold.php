<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Profile\SlowThreshold;

$t = new TestCase('decorator_slow_threshold', 'profiler');

// The `ms` argument is applied per attribute: each function gets its
// own threshold. `slow_decorated` (1 ms threshold, ~150 ms runtime)
// emits a `slow` event; `fast_decorated` (1 s threshold, instant) does
// not. This script is a smoke check on the execution path — the
// emitted span events are asserted in the Rust harness
// (tests/profiler_decorators_tests.rs).
#[SlowThreshold(ms: 1)]
function slow_decorated(): int
{
    usleep(150 * 1000); // 150 ms
    return 1;
}

#[SlowThreshold(ms: 1000)]
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
