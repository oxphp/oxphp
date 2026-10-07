<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Without mime_content_type() — ext/fileinfo not built, or the function in
// disable_functions, as in this profile — the type cannot be detected. moveTo()
// still moves the file, since it does not detect the type, and every type()
// call says what is missing with a warning and returns
// application/octet-stream, without keeping that answer. Sent via curl as
// -F "doc=@small.txt" (contents "hello world").
$t = new TestCase('object_file_without_fileinfo', 'fileinfo_off');

$t->assertFalse('mime_content_type() is not available in this profile', function_exists('mime_content_type'));

$req = oxphp_http_request();
$f = $req->file('doc');
$tmp = $f->tmpPath();

$dest = '/tmp/oxphp_uf_nofileinfo_' . uniqid('', true) . '.txt';
try {
    $moved = $f->moveTo($dest);
} catch (Throwable $e) {
    $moved = get_class($e) . ': ' . $e->getMessage();
}
$t->assertSame('moveTo() moves the file without mime_content_type()', $moved, true);
$t->assertTrue('destination exists after move', is_file($dest));
$t->assertSame('moved content matches the upload', is_file($dest) ? file_get_contents($dest) : '', 'hello world');
$t->assertFalse('the upload is no longer registered, so move_uploaded_file() ran', is_uploaded_file($tmp));

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
}, E_WARNING);
$first = $f->type();
$second = $f->type();
restore_error_handler();

$t->assertSame('type() falls back to octet-stream', $first, 'application/octet-stream');
$t->assertSame('a second call falls back too', $second, 'application/octet-stream');
$t->assertCount('each call warned, so the fallback was not kept', $warnings, 2);
foreach ($warnings as $i => $warning) {
    $t->assertMatch("warning $i names the missing function", $warning, '/mime_content_type\(\) is not available/');
}

// Under an error handler that turns warnings into exceptions, as frameworks
// install (the TestCase one does), the warning escapes type() as an exception.
$t->assertThrows('a throwing error handler turns the warning into an exception', fn () => $f->type(), ErrorException::class);

if (is_file($dest)) {
    unlink($dest);
}

$t->done();
