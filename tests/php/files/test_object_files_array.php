<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('object_files_array', 'files');

$req = oxphp_http_request();

$first = $req->file('photos');
$t->assertInstanceOf('file() returns the first file of an array field', $first, 'OxPHP\Http\UploadedFileInterface');

$photos = $req->files('photos');
$t->assertType('files(name) returns an array', $photos, 'array');
$t->assertCount('files(name) lists all files of the field', $photos, 2);
$t->assertInstanceOf('files(name)[0] is UploadedFileInterface', $photos[0] ?? null, 'OxPHP\Http\UploadedFileInterface');

$t->done();
