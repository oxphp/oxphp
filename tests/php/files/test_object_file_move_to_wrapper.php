<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A stream-wrapper destination: move_uploaded_file() cannot rename to it, so it
// copies through the wrapper, and type() opens the URL again through the
// wrapper. file:// reads the file back; compress.zlib:// reads it back too but
// cannot stat it, which mime_content_type() needs, so type() falls back to
// application/octet-stream (with a warning on PHP 8.4, without one on 8.5).
// Sent via curl as -F "a=@small.txt" -F "b=@small.txt" (contents "hello world").
$t = new TestCase('object_file_move_to_wrapper', 'files');

$req = oxphp_http_request();
$dir = '/tmp/oxphp_uf_wrapper_' . uniqid('', true);
mkdir($dir);
chdir($dir);

$a = $req->file('a');
$t->assertSame('moveTo() to a file:// URL', $a->moveTo('file://' . $dir . '/a.txt'), true);
$t->assertTrue('the file:// destination exists', is_file($dir . '/a.txt'));
try {
    $typeA = $a->type();
} catch (Throwable $e) {
    $typeA = get_class($e) . ': ' . $e->getMessage();
}
$t->assertSame('type() reads the file:// URL back', $typeA, 'text/plain');

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
}, E_WARNING);
$b = $req->file('b');
$movedB = $b->moveTo('compress.zlib://' . $dir . '/b.gz');
$typeB = $b->type();
restore_error_handler();
$t->assertSame('moveTo() to a compress.zlib:// URL', $movedB, true);
$t->assertSame('the gzip destination holds the upload', is_file($dir . '/b.gz') ? gzdecode(file_get_contents($dir . '/b.gz')) : '', 'hello world');
$t->assertSame('type() cannot stat a compress.zlib:// URL', $typeB, 'application/octet-stream');
$t->assertCount('a warning on PHP 8.4 only', $warnings, PHP_VERSION_ID < 80500 ? 1 : 0);
foreach ($warnings as $warning) {
    // Not a failed open, which would warn on both versions.
    $t->assertMatch('the PHP 8.4 warning is the failed stat', $warning, '/Failed identify data/');
}

foreach (['a.txt', 'b.gz'] as $name) {
    if (is_file($dir . '/' . $name)) {
        unlink($dir . '/' . $name);
    }
}
rmdir($dir);

$t->done();
