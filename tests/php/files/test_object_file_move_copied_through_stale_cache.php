<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A copy that resolves the destination through a stale realpath cache entry.
// move_uploaded_file(), called directly, renames one upload over a symlink
// whose resolution the cache holds, and leaves the entry in place. moveTo()
// then moves another upload to the same name in a directory the rename cannot
// write, so move_uploaded_file() copies, and the copy follows the stale entry
// to the old link target. type() must read the file the copy wrote, not the
// one now at the destination name.
// Sent via curl as -F "a=@small.txt" -F "b=@image.png" (text and a PNG).
$t = new TestCase('object_file_move_copied_through_stale_cache', 'files');

$req = oxphp_http_request();
$dir = '/tmp/oxphp_uf_stale_' . uniqid('', true);
mkdir($dir);

$typeOf = static function ($f): string {
    try {
        return $f->type();
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};

file_put_contents($dir . '/logo.gif', 'GIF89a');
symlink($dir . '/logo.gif', $dir . '/a.php');
$t->assertTrue('the link resolves to the GIF', is_file($dir . '/a.php'));

$a = $req->file('a');
$t->assertTrue('move_uploaded_file() renames over the link', move_uploaded_file($a->tmpPath(), $dir . '/a.php'));
$t->assertFalse('the link was replaced', is_link($dir . '/a.php'));
chmod($dir, 0555);
$t->assertFalse('the rename cannot write into the directory', is_writable($dir));
$cached = realpath_cache_get()[$dir . '/a.php']['realpath'] ?? '';
$t->assertSame('the cache still resolves the name to the old link target', $cached, $dir . '/logo.gif');

$b = $req->file('b');
$t->assertSame('moveTo() to the same name', $b->moveTo($dir . '/a.php'), true);
$t->assertSame('type() reads the file the copy wrote', $typeOf($b), 'image/png');

clearstatcache(true);
$t->assertSame('the copy wrote to the old link target', mime_content_type($dir . '/logo.gif'), 'image/png');
$t->assertSame('the file at the destination name is the first upload', file_get_contents($dir . '/a.php'), 'hello world');

chmod($dir, 0755);
foreach (['a.php', 'logo.gif'] as $name) {
    if (is_file($dir . '/' . $name) || is_link($dir . '/' . $name)) {
        unlink($dir . '/' . $name);
    }
}
rmdir($dir);

$t->done();
