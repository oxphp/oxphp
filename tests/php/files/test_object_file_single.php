<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('object_file_single', 'files');

$req = oxphp_http_request();
$f = $req->file('avatar');

$t->assertInstanceOf('file() returns UploadedFileInterface', $f, 'OxPHP\Http\UploadedFileInterface');
$t->assertTrue('isValid() is true', $f->isValid());
$t->assertSame('error() is UPLOAD_ERR_OK', $f->error(), UPLOAD_ERR_OK);
$t->assertSame('name() is the original client filename', $f->name(), 'small.txt');
$t->assertGreaterThan('size() > 0', $f->size(), 0);
$t->assertTrue('tmpPath() points at the uploaded temp file', is_uploaded_file($f->tmpPath()));
$t->assertNull('file() on a missing field returns null', $req->file('nonexistent'));

$t->done();
