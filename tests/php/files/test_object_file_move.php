<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('object_file_move', 'files');

$req = oxphp_http_request();
$f = $req->file('doc');

$t->assertInstanceOf('file() returns UploadedFileInterface', $f, 'OxPHP\Http\UploadedFileInterface');
$t->assertTrue('isValid() before move', $f->isValid());

$dest = '/tmp/oxphp_uf_move_' . uniqid('', true) . '.txt';
$t->assertTrue('moveTo() returns true for a valid upload', $f->moveTo($dest));
$t->assertTrue('destination exists after move', is_file($dest));

$content = is_file($dest) ? file_get_contents($dest) : '';
$t->assertSame('moved content matches the upload', $content, 'hello world');

// The temp file no longer exists after the move; type() can only still report
// the real MIME because moveTo() detected and cached it before calling
// move_uploaded_file(). Without that pre-cache it would fall back here.
$t->assertNotEqual('type() returns the detected MIME after move (cached)', $f->type(), 'application/octet-stream');

if (is_file($dest)) {
    unlink($dest);
}

$t->done();
