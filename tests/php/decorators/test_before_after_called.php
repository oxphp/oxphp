<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('before_after_called', 'decorators');

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class LogDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public static array $log = [];

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'before';
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'after';
    }
}

oxphp_register_decorator(LogDecorator::class);

#[LogDecorator]
function decorated_target(): string
{
    LogDecorator::$log[] = 'called';
    return 'ok';
}

decorated_target();

$t->assertSame('before was called first', LogDecorator::$log[0], 'before');
$t->assertSame('target was called second', LogDecorator::$log[1], 'called');
$t->assertSame('after was called third', LogDecorator::$log[2], 'after');
$t->assertCount('log has 3 entries', LogDecorator::$log, 3);

$t->done();
