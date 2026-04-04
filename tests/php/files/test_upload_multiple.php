<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_multiple', 'files');

$t->assertCount('$_FILES has 2 entries', $_FILES, 2);

foreach (['file1', 'file2'] as $key) {
    $t->assertKeyExists("$key exists in \$_FILES", $_FILES, $key);
    if (array_key_exists($key, $_FILES)) {
        $t->assertSame("$key error is 0", $_FILES[$key]['error'], UPLOAD_ERR_OK);
    }
}

$t->done();
