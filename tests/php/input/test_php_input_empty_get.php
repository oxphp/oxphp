<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('php_input_empty_get', 'input');

$body = file_get_contents('php://input');
$t->assertSame('php://input is empty string for GET', $body, '');

$t->done();
