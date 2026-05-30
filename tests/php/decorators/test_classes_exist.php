<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('classes_exist', 'decorators');

$t->assertTrue(
    "interface_exists('OxPHP\\Decorator\\AttributeInterface')",
    interface_exists(\OxPHP\Decorator\AttributeInterface::class)
);
$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\Context')",
    class_exists(\OxPHP\Decorator\Context::class)
);
$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\RejectedException')",
    class_exists(\OxPHP\Decorator\RejectedException::class)
);
$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\StackOverflowException')",
    class_exists(\OxPHP\Decorator\StackOverflowException::class)
);

$t->done();
