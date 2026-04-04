<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('dispatch_order', 'decorators');

#[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::IS_REPEATABLE)]
class OrderDecoratorA implements \OxPHP\Decorator\AttributeInterface
{
    public static array $log = [];

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'A:before';
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'A:after';
    }
}

#[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::IS_REPEATABLE)]
class OrderDecoratorB implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        OrderDecoratorA::$log[] = 'B:before';
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        OrderDecoratorA::$log[] = 'B:after';
    }
}

oxphp_register_decorator(OrderDecoratorA::class);
oxphp_register_decorator(OrderDecoratorB::class);

#[OrderDecoratorA]
#[OrderDecoratorB]
function ordered_target(): void
{
    OrderDecoratorA::$log[] = 'called';
}

ordered_target();

$log = OrderDecoratorA::$log;

// before: A then B; after: B then A (reverse)
$t->assertSame('before order: A first', $log[0], 'A:before');
$t->assertSame('before order: B second', $log[1], 'B:before');
$t->assertSame('target called third', $log[2], 'called');
$t->assertSame('after order: B first (reverse)', $log[3], 'B:after');
$t->assertSame('after order: A last (reverse)', $log[4], 'A:after');

$t->done();
