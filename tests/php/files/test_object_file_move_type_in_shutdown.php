<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A relative destination, and type() first called from a shutdown function:
// PHP changes the working directory back once the script body has run, before
// shutdown functions and destructors. type() must still find the moved file.
// Sent via curl as -F "doc=@small.txt" (contents "hello world").
$t = new TestCase('object_file_move_type_in_shutdown', 'files');

$f = oxphp_http_request()->file('doc');

$dir = '/tmp/oxphp_uf_shutdown_' . uniqid('', true);
mkdir($dir);
chdir($dir);

try {
    $moved = $f->moveTo('moved.txt');
} catch (Throwable $e) {
    $moved = get_class($e) . ': ' . $e->getMessage();
}
$t->assertSame('moveTo() moves the file', $moved, true);
$t->assertTrue('the moved file is in the working directory', is_file($dir . '/moved.txt'));

register_shutdown_function(static function () use ($t, $f, $dir): void {
    // The premise: the working directory is no longer the one moveTo() saw.
    $t->assertTrue('the working directory changed before shutdown', getcwd() !== $dir);

    try {
        $type = $f->type();
    } catch (Throwable $e) {
        $type = get_class($e) . ': ' . $e->getMessage();
    }
    $t->assertSame('type() in a shutdown function finds the moved file', $type, 'text/plain');

    if (is_file($dir . '/moved.txt')) {
        unlink($dir . '/moved.txt');
    }
    rmdir($dir);

    $t->done();
});
