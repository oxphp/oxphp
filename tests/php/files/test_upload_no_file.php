<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_no_file', 'files');

$t->assertSame('$_FILES is empty array', $_FILES, []);

$t->done();
