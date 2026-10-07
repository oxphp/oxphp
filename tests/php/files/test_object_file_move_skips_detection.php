<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// moveTo() moves the file the way move_uploaded_file() does, without detecting
// the type on the way. Detection reads the temporary file, and here it
// cannot — open_basedir covers only the destination directory, so
// mime_content_type() refuses the temporary file with a warning, which the
// TestCase error handler turns into an exception, as frameworks do.
// move_uploaded_file() checks open_basedir only against the destination, so the
// move itself is allowed. Sent via curl as -F "doc=@small.txt" (contents
// "hello world").
$t = new TestCase('object_file_move_skips_detection', 'files');

$req = oxphp_http_request();
$f = $req->file('doc');
$tmp = $f->tmpPath();

$dir = '/tmp/oxphp_uf_obd_' . uniqid('', true);
mkdir($dir);
ini_set('open_basedir', $dir);

// The premise: detecting the type of the temporary file fails in this setup.
try {
    $premise = 'no exception, returned ' . $req->file('doc')->type();
} catch (ErrorException $e) {
    $premise = $e->getMessage();
}
$t->assertMatch('detecting the type of the temporary file fails', $premise, '/open_basedir restriction/');

$dest = $dir . '/moved.txt';
try {
    $moved = $f->moveTo($dest);
} catch (Throwable $e) {
    $moved = get_class($e) . ': ' . $e->getMessage();
}
$t->assertSame('moveTo() moves the file without detecting its type', $moved, true);
$t->assertTrue('destination exists after move', is_file($dest));
$t->assertSame('moved content matches the upload', is_file($dest) ? file_get_contents($dest) : '', 'hello world');
$t->assertFalse('the upload is no longer registered, so move_uploaded_file() ran', is_uploaded_file($tmp));

// The object that moved the file detects its type at the destination.
$t->assertSame('type() reads the file where moveTo() put it', $f->type(), 'text/plain');

if (is_file($dest)) {
    unlink($dest);
}
rmdir($dir);

$t->done();
