<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('rejected_exception', 'decorators');

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class RejectingDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        throw new \OxPHP\Decorator\RejectedException('rejected by decorator');
    }

    public function after(\OxPHP\Decorator\Context $ctx): void {}
}

oxphp_register_decorator(RejectingDecorator::class);

$called = false;

#[RejectingDecorator]
function rejectable_target(): void
{
    global $called;
    $called = true;
}

try {
    rejectable_target();
} catch (\OxPHP\Decorator\RejectedException $e) {
    // expected
}

$t->assertFalse('target function was NOT called after RejectedException', $called);

$t->done();
