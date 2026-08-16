<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('php_input_raw', 'input');

$body = file_get_contents('php://input');
$t->assertContains('php://input contains "hello"', (string)$body, 'hello');

$t->done();
