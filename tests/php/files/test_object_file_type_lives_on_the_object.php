<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// type() keeps what it detected on the UploadedFile it was called on, not on the
// uploaded file. file() and files() expand the $_FILES entry into new objects on
// every call, so each object starts without a type and reads the file itself on
// its first type() call. Two consequences are pinned here: a second object reads
// the file again, and once moveTo() has moved the file the other objects that
// did not detect before the move find no file. Sent via curl as
// -F "doc=@small.txt" (contents "hello world").
$t = new TestCase('object_file_type_lives_on_the_object', 'files');

$req = oxphp_http_request();

// One uploaded file, a new object per call.
$a = $req->file('doc');
$b = $req->file('doc');
$t->assertTrue('two file() calls return two objects', $a !== $b);
$t->assertSame('both point at the same uploaded file', $a->tmpPath(), $b->tmpPath());
$t->assertTrue('two files() calls return two objects', $req->files('doc')[0] !== $req->files('doc')[0]);

// The first call reads the file; later calls on the same object do not. Changing
// the file under it shows which object reads and which answers from its own copy.
$mime = $a->type();
$t->assertSame('type() detects the uploaded contents', $mime, 'text/plain');
file_put_contents($a->tmpPath(), "GIF89a\x01\x00\x01\x00\x80\x00\x00");
$t->assertSame('the object that detected answers without reading again', $a->type(), $mime);
$c = $req->file('doc');
$t->assertSame('a new object reads the file again', $c->type(), 'image/gif');

// After the move the temporary file is gone. The objects that detected before it
// keep their answer; one that did not — taken before the move or fetched after
// it — finds no file, and mime_content_type() warns about it.
$dest = '/tmp/oxphp_uf_type_' . uniqid('', true);
$t->assertTrue('moveTo() returns true for a valid upload', $a->moveTo($dest));
$t->assertSame('the moved object still reports its type', $a->type(), $mime);
$t->assertSame('another object that detected before the move keeps its own', $c->type(), 'image/gif');

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
}, E_WARNING);
$before = $b->type();
$after = $req->file('doc')->type();
restore_error_handler();

$t->assertSame('an object taken before the move but never asked falls back', $before, 'application/octet-stream');
$t->assertSame('an object fetched after the move falls back', $after, 'application/octet-stream');
$t->assertCount('each of them ran detection and warned', $warnings, 2);
foreach ($warnings as $i => $warning) {
    $t->assertMatch("warning $i comes from mime_content_type()", $warning, '/^mime_content_type\(/');
}

// Under an error handler that turns warnings into exceptions, as frameworks
// install, that warning escapes type() as an exception instead of a return value.
set_error_handler(static function (int $errno, string $errstr): never {
    throw new ErrorException($errstr, 0, $errno);
}, E_WARNING);
$late = $req->file('doc');
$t->assertThrows('a throwing error handler turns the fallback into an exception', fn () => $late->type(), ErrorException::class);
restore_error_handler();

if (is_file($dest)) {
    unlink($dest);
}

$t->done();
