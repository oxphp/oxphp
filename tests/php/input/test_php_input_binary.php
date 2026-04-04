<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('php_input_binary', 'input');

$body = file_get_contents('php://input');
$t->assertSame('strlen matches expected body length', strlen((string)$body), strlen('binary_data_here'));
$t->assertSame('body content matches', $body, 'binary_data_here');

$t->done();
