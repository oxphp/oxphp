<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('object_files_all', 'files');

$req = oxphp_http_request();
$files = $req->files();

$t->assertType('files() returns an array', $files, 'array');
$t->assertCount('files() lists every uploaded file', $files, 2);
foreach ($files as $f) {
    $t->assertInstanceOf('each entry is UploadedFileInterface', $f, 'OxPHP\Http\UploadedFileInterface');
}

$t->done();
