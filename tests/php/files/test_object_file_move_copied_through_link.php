<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A destination through a symlinked directory and back with "..", in a
// directory the rename cannot write, so move_uploaded_file() falls back to a
// copy. The copy opens the destination following the link first, and writes
// next to the link's target rather than where ".." taken by name points.
// type() must read the file the copy wrote:
// - b: a file already sits where ".." taken by name points;
// - a: the destination is relative, and type() is first called from a
//   shutdown function, after PHP has changed the working directory back.
// Sent via curl as -F "a=@small.txt" -F "b=@small.txt" (contents "hello world").
$t = new TestCase('object_file_move_copied_through_link', 'files');

$req = oxphp_http_request();
$gif = 'GIF89a';
$dir = '/tmp/oxphp_uf_copy_' . uniqid('', true);
mkdir($dir);
mkdir($dir . '/up');
mkdir($dir . '/data');
mkdir($dir . '/data/x');
symlink($dir . '/data/x', $dir . '/up/link');
file_put_contents($dir . '/up/b.txt', $gif);
chmod($dir . '/up', 0555);
$t->assertFalse('the rename cannot write into up/', is_writable($dir . '/up'));

$typeOf = static function ($f): string {
    try {
        return $f->type();
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};
$contents = static fn (string $path): string => is_file($path) ? file_get_contents($path) : '';

$b = $req->file('b');
$t->assertSame('moveTo() through link/.. onto a taken name', $b->moveTo($dir . '/up/link/../b.txt'), true);
$t->assertSame('the copy wrote next to the link target', $contents($dir . '/data/b.txt'), 'hello world');
$t->assertSame('the file where ".." by name points is untouched', $contents($dir . '/up/b.txt'), $gif);
$t->assertSame('type() reads the file the copy wrote', $typeOf($b), 'text/plain');

chdir($dir);
$a = $req->file('a');
$t->assertSame('moveTo() through link/.. by a relative path', $a->moveTo('up/link/../a.txt'), true);
$t->assertSame('the copy wrote next to the link target', $contents($dir . '/data/a.txt'), 'hello world');

register_shutdown_function(static function () use ($t, $a, $dir, $typeOf): void {
    $t->assertTrue('the working directory changed before shutdown', getcwd() !== $dir);
    $t->assertSame('type() from shutdown reads the file the copy wrote', $typeOf($a), 'text/plain');

    chmod($dir . '/up', 0755);
    foreach (['up/link', 'up/b.txt', 'data/a.txt', 'data/b.txt'] as $name) {
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
});
