<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('register_decorator', 'decorators');

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class TestDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void {}
    public function after(\OxPHP\Decorator\Context $ctx): void {}
}

$result = oxphp_register_decorator(TestDecorator::class);
$t->assertTrue('oxphp_register_decorator() returns true', $result === true);

$t->done();
