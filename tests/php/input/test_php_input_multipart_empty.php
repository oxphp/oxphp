<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('php_input_multipart_empty', 'input');

// PHP standard behavior: php://input is not available for multipart/form-data
// because PHP consumes the body during $_POST/$_FILES population.
$body = file_get_contents('php://input');
$t->assertSame('php://input is empty for multipart POST', $body, '');

$t->done();
