<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('rejected_exception', 'decorators');

// When `before()` throws a RejectedException, OxPHP records the
// exception on EG(exception) before the decorated function body starts
// executing. PHP's Zend observer API (zend_observer_fcall_begin) does
// not expose a way to cancel the call itself, so the decorated function
// may still run a handful of opcodes before the VM unwinds. The
// contract we guarantee is:
//   1. `before()` runs and may throw RejectedException.
//   2. The exception propagates out of the decorated call and is
//      catchable by the caller with a normal try/catch.
//   3. `after()` is NOT invoked on a decorator whose `before()` threw.
//
// Tests below pin down those guarantees.

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class RejectingDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public static int $afterCalls = 0;

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        throw new \OxPHP\Decorator\RejectedException('rejected by decorator');
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        self::$afterCalls++;
    }
}

oxphp_register_decorator(RejectingDecorator::class);

#[RejectingDecorator]
function rejectable_target(): string
{
    return 'ok';
}

$caught = null;
try {
    rejectable_target();
} catch (\OxPHP\Decorator\RejectedException $e) {
    $caught = $e;
}

$t->assertNotNull('RejectedException was caught by caller', $caught);
$t->assertSame(
    'exception carries message from before()',
    $caught?->getMessage(),
    'rejected by decorator'
);
$t->assertSame(
    'after() is NOT called when before() threw',
    RejectingDecorator::$afterCalls,
    0
);

$t->done();
