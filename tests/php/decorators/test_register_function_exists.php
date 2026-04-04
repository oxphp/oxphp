<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('register_function_exists', 'decorators');

$t->assertTrue(
    "function_exists('oxphp_register_decorator') === true",
    function_exists('oxphp_register_decorator') === true
);

$t->done();
