<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

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

$tmp = $f->tmpPath();
$t->assertFalse('the upload is no longer registered after the move', is_uploaded_file($tmp));

// moveTo() does not detect the type; the object that moved the file reads it at
// the destination on its first type() call. Changing the file there before that
// call shows which file is read.
file_put_contents($dest, "GIF89a\x01\x00\x01\x00\x80\x00\x00");
$t->assertSame('type() reads the file at the destination after the move', $f->type(), 'image/gif');

// Only a successful move points type() at the destination. A second object for
// the same upload cannot move it again — it is no longer registered — and still
// reads the temporary file, which is gone, rather than what is at $dest.
$g = $req->file('doc');
$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
}, E_WARNING);
$movedAgain = $g->moveTo($dest);
$other = $g->type();
restore_error_handler();

$t->assertFalse('a second move of the same upload fails', $movedAgain);
$t->assertSame('a failed move leaves type() on the temporary file', $other, 'application/octet-stream');
$t->assertCount('only detection of the missing temporary file warned', $warnings, 1);
$t->assertMatch('the warning comes from mime_content_type()', $warnings[0] ?? '', '/^mime_content_type\(/');

if (is_file($dest)) {
    unlink($dest);
}

$t->done();
