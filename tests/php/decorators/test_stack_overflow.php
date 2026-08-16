<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stack_overflow', 'decorators');

// The decorator context stack is a fixed-size per-thread buffer
// (OXPHP_DECORATOR_CTX_STACK_MAX = 256). Beyond that depth, OxPHP must fail
// loud — throw OxPHP\Decorator\StackOverflowException — instead of silently
// reusing the top slot and corrupting outer frames' context.
//
// The thrown exception propagates out of the deepest decorated call and is
// catchable by the caller with a normal try/catch, exactly like
// RejectedException.

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class CountingDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public static int $beforeCalls = 0;
    public static int $afterCalls = 0;

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        self::$beforeCalls++;
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        self::$afterCalls++;
    }
}

oxphp_register_decorator(CountingDecorator::class);

#[CountingDecorator]
function ox_recurse_decorated(int $depth): int
{
    if ($depth <= 0) {
        return 0;
    }
    return ox_recurse_decorated($depth - 1) + 1;
}

// Sanity: a safe nesting depth (well under 256) returns normally and never
// throws — the limit must not fire for ordinary nesting. On the non-throwing
// path every before() is matched by an after(), so the counters stay equal.
CountingDecorator::$beforeCalls = 0;
CountingDecorator::$afterCalls = 0;
$safe = null;
$safeError = null;
try {
    $safe = ox_recurse_decorated(10);
} catch (\Throwable $e) {
    $safeError = $e;
}
$t->assertNull('safe depth (10) does not throw', $safeError);
$t->assertSame('safe depth returns correct result', $safe, 10);
$t->assertGreaterThan('decorators actually ran at safe depth', CountingDecorator::$beforeCalls, 0);
$t->assertSame(
    'before()/after() balanced on the non-throwing path',
    CountingDecorator::$afterCalls,
    CountingDecorator::$beforeCalls
);

// Overflow: nesting past 256 decorated calls must throw StackOverflowException,
// caught here by the top-level caller.
CountingDecorator::$beforeCalls = 0;
CountingDecorator::$afterCalls = 0;
$caught = null;
try {
    ox_recurse_decorated(300);
} catch (\OxPHP\Decorator\StackOverflowException $e) {
    $caught = $e;
}

$t->assertNotNull('StackOverflowException thrown beyond 256 nesting levels', $caught);
$t->assertInstanceOf(
    'caught exception is StackOverflowException',
    $caught,
    \OxPHP\Decorator\StackOverflowException::class
);
$t->assertContains(
    'exception message describes the overflow',
    $caught?->getMessage() ?? '',
    'stack overflow'
);

// before() ran for every frame that fit under the limit (proving the overflow
// path executed deep, not a spurious early throw). after() is suppressed while
// the StackOverflowException unwinds — same contract as RejectedException,
// where after() is not dispatched on a frame with a pending exception.
$t->assertGreaterThan('before() ran for many frames before the overflow', CountingDecorator::$beforeCalls, 200);
$t->assertSame(
    'after() not dispatched while the overflow exception is pending',
    CountingDecorator::$afterCalls,
    0
);

$t->done();
