<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Destinations that resolve differently through a symlink. type() must read the
// file the move created, which the rename put at the destination taken by name.
// - A symlink to a GIF sits at the destination and the realpath cache holds
//   its resolution: the rename replaces the link, but move_uploaded_file()
//   leaves the cache alone, so anything that resolves the path through the
//   cache still lands on the GIF.
// - The destination runs through a symlinked directory and back with "..":
//   the rename takes ".." by name, while resolving the path follows the link
//   first and lands next to its target, where another GIF sits.
// Sent via curl as -F "a=@small.txt" -F "b=@small.txt" (contents "hello world").
// The GIF signature alone is enough for image/gif and keeps every value ASCII:
// a failed assertion with raw bytes in it would empty the whole JSON report.
$t = new TestCase('object_file_move_symlinked_destination', 'files');

$req = oxphp_http_request();
$gif = 'GIF89a';
$dir = '/tmp/oxphp_uf_link_' . uniqid('', true);
mkdir($dir);

$typeOf = static function ($f): string {
    try {
        return $f->type();
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};

// A symlink replaced by the move, with its resolution in the realpath cache.
file_put_contents($dir . '/logo.gif', $gif);
symlink($dir . '/logo.gif', $dir . '/a.php');
$t->assertTrue('the link resolves to the GIF', is_file($dir . '/a.php'));
$t->assertTrue('the realpath cache holds the link', array_key_exists($dir . '/a.php', realpath_cache_get()));

$a = $req->file('a');
$t->assertSame('moveTo() over the link', $a->moveTo($dir . '/a.php'), true);
$t->assertSame('type() reads the moved file, not the old link target', $typeOf($a), 'text/plain');
$t->assertFalse('the link was replaced', is_link($dir . '/a.php'));
$t->assertSame('the moved file holds the upload', file_get_contents($dir . '/a.php'), 'hello world');
$t->assertSame('the old link target is untouched', file_get_contents($dir . '/logo.gif'), $gif);

// ".." after a symlinked directory.
mkdir($dir . '/up');
mkdir($dir . '/data');
mkdir($dir . '/data/x');
symlink($dir . '/data/x', $dir . '/up/link');
file_put_contents($dir . '/data/f.txt', $gif);

$b = $req->file('b');
$t->assertSame('moveTo() through link/..', $b->moveTo($dir . '/up/link/../f.txt'), true);
$t->assertSame('type() reads the moved file, not the one next to the link target', $typeOf($b), 'text/plain');
$t->assertSame('the rename took ".." by name', is_file($dir . '/up/f.txt') ? file_get_contents($dir . '/up/f.txt') : '', 'hello world');
$t->assertSame('the file next to the link target is untouched', file_get_contents($dir . '/data/f.txt'), $gif);

foreach (['a.php', 'logo.gif', 'up/f.txt', 'up/link', 'data/f.txt'] as $name) {
    if (is_file($dir . '/' . $name) || is_link($dir . '/' . $name)) {
        unlink($dir . '/' . $name);
    }
}
foreach (['data/x', 'data', 'up', ''] as $name) {
    if (is_dir($dir . '/' . $name)) {
        rmdir($dir . '/' . $name);
    }
}

$t->done();
