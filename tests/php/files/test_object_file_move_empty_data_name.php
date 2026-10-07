<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An empty upload whose client file name reads as a data: URL, moved by that
// name into a directory the server cannot write, where a GIF of that name
// already sits. The rename fails, so move_uploaded_file() copies through a
// stream wrapper instead: the data: wrapper, which cannot write, but a copy of
// zero bytes never asks it to, so the move reports success with nothing
// written. type() must find no file rather than detect the type the name
// spells out, or read the file that was there before.
// Sent via curl as -F "doc=@/dev/null;filename=\"data:,GIF89a.php\"".
$t = new TestCase('object_file_move_empty_data_name', 'files');

$req = oxphp_http_request();
$f = $req->file('doc');
$t->assertSame('the client file name arrives as sent', $f->name(), 'data:,GIF89a.php');
$t->assertSame('the upload is empty', $f->size(), 0);
$t->assertSame('read as a data: URL, the name is a GIF', mime_content_type($f->name()), 'image/gif');

$dir = '/tmp/oxphp_uf_dataname_' . uniqid('', true);
mkdir($dir);
file_put_contents($dir . '/data:,GIF89a.php', 'GIF89a');
chmod($dir, 0555);
$t->assertFalse('the rename cannot write into the directory', is_writable($dir));

chdir($dir);
$t->assertSame('moveTo() reports success, as move_uploaded_file() does', $f->moveTo(basename($f->name())), true);
$t->assertSame('nothing was written over the file at the name', file_get_contents($dir . '/data:,GIF89a.php'), 'GIF89a');

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
}, E_WARNING);
$type = $f->type();
restore_error_handler();
$t->assertSame('type() finds no file instead of reading the name or the file there', $type, 'application/octet-stream');
$t->assertCount('one warning', $warnings, 1);
foreach ($warnings as $warning) {
    $t->assertMatch('the warning is for the missing file', $warning, '/Failed to open stream/');
}

chmod($dir, 0755);
unlink($dir . '/data:,GIF89a.php');
rmdir($dir);

$t->done();
