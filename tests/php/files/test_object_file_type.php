<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('object_file_type', 'files');

$req = oxphp_http_request();
$f = $req->file('doc');

$t->assertInstanceOf('file() returns UploadedFileInterface', $f, 'OxPHP\Http\UploadedFileInterface');
$t->assertSame('clientType() reflects the client-declared type', $f->clientType(), 'application/x-bogus');
$t->assertType('type() returns a string', $f->type(), 'string');
$t->assertNotEqual('type() is detected from contents, not the client claim', $f->type(), 'application/x-bogus');

$t->done();
