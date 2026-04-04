<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_content', 'files');

$t->assertNotEmpty('$_FILES is not empty', $_FILES);

$file = reset($_FILES);
$t->assertSame('upload error is 0', $file['error'], UPLOAD_ERR_OK);

$dest = '/tmp/oxphp_upload_test_' . uniqid('', true);
$moved = move_uploaded_file($file['tmp_name'], $dest);
$t->assertTrue('move_uploaded_file succeeded', $moved);

if ($moved) {
    $content = file_get_contents($dest);
    $t->assertSame('content matches expected', $content, 'hello world');
    unlink($dest);
}

$t->done();
