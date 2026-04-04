<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('classes_exist', 'decorators');

$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\AttributeInterface')",
    class_exists(\OxPHP\Decorator\AttributeInterface::class)
);
$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\Context')",
    class_exists(\OxPHP\Decorator\Context::class)
);
$t->assertTrue(
    "class_exists('OxPHP\\Decorator\\RejectedException')",
    class_exists(\OxPHP\Decorator\RejectedException::class)
);

$t->done();
