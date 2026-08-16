<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A file input present in the form but with no file chosen (empty filename)
// still produces a $_FILES entry — error=UPLOAD_ERR_NO_FILE, tmp_name="",
// size=0. file() must surface it as a non-null but invalid UploadedFile, and
// moveTo() must refuse it. Sent via curl as -F "avatar=@/dev/null;filename=".
$t = new TestCase('object_file_nofile', 'files');

$req = oxphp_http_request();
$f = $req->file('avatar');

$t->assertInstanceOf('present-but-empty field still yields an UploadedFile', $f, 'OxPHP\Http\UploadedFileInterface');
$t->assertFalse('isValid() is false when no file was chosen', $f->isValid());
$t->assertSame('error() is UPLOAD_ERR_NO_FILE', $f->error(), UPLOAD_ERR_NO_FILE);
$t->assertSame('size() is 0', $f->size(), 0);

// type() must fall back cleanly for an empty upload, not throw ValueError from
// mime_content_type('') — moveTo() pre-caches the MIME and would otherwise blow up.
$t->assertSame('type() falls back to octet-stream for an empty upload', $f->type(), 'application/octet-stream');

$dest = '/tmp/oxphp_uf_nofile_' . uniqid('', true);
$t->assertFalse('moveTo() refuses an invalid upload', $f->moveTo($dest));
$t->assertFalse('nothing was written to the destination', is_file($dest));

$t->done();
