<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_no_file', 'files');

$t->assertSame('$_FILES is empty array', $_FILES, []);

$t->done();
