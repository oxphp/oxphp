<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('php_input_rewindable', 'input');

$first  = file_get_contents('php://input');
$second = file_get_contents('php://input');

$t->assertSame('second read matches first read', $second, $first);
$t->assertNotEmpty('body is not empty', $first);

$t->done();
