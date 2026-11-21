<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('cleanup_after_throwing_destructor', 'decorators');

// When a later decorator's before() throws, the decorators whose before()
// already ran get their after() as cleanup, and whatever that cleanup throws is
// dropped in favour of the exception the caller is owed. Dropping it is user
// code too: its destructor runs and can throw in its turn. The caller still
// gets the exception before() threw, not the destructor's.

class CleanupExceptionWithThrowingDestructor extends \RuntimeException
{
    public function __destruct()
    {
        throw new \LogicException('thrown by the dropped exception\'s destructor');
    }
}

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class CleanupOuterDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void
    {
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        throw new CleanupExceptionWithThrowingDestructor('thrown by after()');
    }
}

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class CleanupInnerDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        throw new \OxPHP\Decorator\RejectedException('original');
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
    }
}

oxphp_register_decorator(CleanupOuterDecorator::class);
oxphp_register_decorator(CleanupInnerDecorator::class);

#[CleanupOuterDecorator]
#[CleanupInnerDecorator]
function cleanup_target(): string
{
    return 'ok';
}

$caught = null;
try {
    cleanup_target();
} catch (\Throwable $e) {
    $caught = $e;
}

$t->assertInstanceOf('the caller gets the exception before() threw', $caught, \OxPHP\Decorator\RejectedException::class);
$t->assertSame('with its own message', $caught?->getMessage(), 'original');
$t->done();
