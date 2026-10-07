<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The working directory changes while move_uploaded_file() runs. After the
// rename it sets the moved file's permissions, and the chmod goes through the
// realpath cache, which still resolves the destination to the symlink the
// rename replaced, here one to a file the server cannot chmod. The warning
// that raises runs the error handler before move_uploaded_file() returns, and
// the handler changes the working directory to one holding a GIF under the
// destination's relative name. type() must read the file the rename moved.
// Sent via curl as -F "doc=@small.txt" (contents "hello world").
$t = new TestCase('object_file_move_cwd_changed_during_move', 'files');

$req = oxphp_http_request();
$dir = '/tmp/oxphp_uf_cwd_' . uniqid('', true);
$other = $dir . '_other';
mkdir($dir);
mkdir($other);
file_put_contents($other . '/a.php', 'GIF89a');

$target = '/etc/passwd';
$t->assertFalse('the server cannot write the link target', is_writable($target));
if (is_writable($target)) {
    $t->done();
}
symlink($target, $dir . '/a.php');
$t->assertSame('the link resolves to the target', realpath($dir . '/a.php'), $target);
$cached = realpath_cache_get()[$dir . '/a.php']['realpath'] ?? '';
$t->assertSame('the realpath cache holds the link', $cached, $target);

chdir($dir);
$f = $req->file('doc');
$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings, $other): bool {
    $warnings[] = $errstr;
    chdir($other);
    return true;
}, E_WARNING);
$moved = $f->moveTo('a.php');
restore_error_handler();

$t->assertSame('moveTo() over the link by a relative path', $moved, true);
$t->assertCount('one warning, from the chmod after the rename', $warnings, 1);
foreach ($warnings as $warning) {
    $t->assertMatch('the chmod went to the link target', $warning, '/Operation not permitted/');
}
$t->assertSame('the handler changed the working directory', getcwd(), $other);
$t->assertSame('the rename replaced the link', is_link($dir . '/a.php'), false);
$t->assertSame('the moved file holds the upload', file_get_contents($dir . '/a.php'), 'hello world');
$t->assertSame('type() reads the moved file, not the one in the new directory', $f->type(), 'text/plain');

foreach ([$dir, $other] as $d) {
    unlink($d . '/a.php');
    rmdir($d);
}

$t->done();
