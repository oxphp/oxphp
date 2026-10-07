<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A relative destination built from the client's file name, here
// "data:,GIF89a.php": move_uploaded_file() renames the upload to a local file
// of that name, but PHP's streams read a name starting with "data:" as a data:
// URL. type() must detect the type of the moved file, not of the text in its
// name. Sent via curl as -F "doc=@small.txt;filename=..." (contents
// "hello world").
$t = new TestCase('object_file_move_data_name', 'files');

$f = oxphp_http_request()->file('doc');
$t->assertSame('the client file name reaches name() intact', $f->name(), 'data:,GIF89a.php');
// The premise: opened by that name, the stream is the text after the comma.
$t->assertSame('the name read as a data: URL types as GIF', mime_content_type('data:,GIF89a.php'), 'image/gif');

$dir = '/tmp/oxphp_uf_data_' . uniqid('', true);
mkdir($dir);
chdir($dir);

try {
    $moved = $f->moveTo(basename($f->name()));
} catch (Throwable $e) {
    $moved = get_class($e) . ': ' . $e->getMessage();
}
$dest = $dir . '/data:,GIF89a.php';
$t->assertSame('moveTo() moves the file', $moved, true);
$t->assertSame('the moved file is a local file of that name', is_file($dest) ? file_get_contents($dest) : '', 'hello world');

try {
    $type = $f->type();
} catch (Throwable $e) {
    $type = get_class($e) . ': ' . $e->getMessage();
}
$t->assertSame('type() detects the moved file, not its name', $type, 'text/plain');

if (is_file($dest)) {
    unlink($dest);
}
rmdir($dir);

$t->done();
