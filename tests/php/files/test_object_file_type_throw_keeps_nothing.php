<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A type() call that threw detected nothing, so it keeps nothing: the next call
// on the same object reads the file again. A detection that fails without
// throwing does keep its answer. open_basedir here leaves the temporary file
// out of reach, so mime_content_type() refuses it with a warning, which the
// TestCase error handler turns into an exception, as frameworks do. Sent via
// curl as -F "doc=@small.txt".
$t = new TestCase('object_file_type_throw_keeps_nothing', 'files');

$f = oxphp_http_request()->file('doc');

$dir = '/tmp/oxphp_uf_throw_' . uniqid('', true);
mkdir($dir);
ini_set('open_basedir', $dir);

try {
    $first = 'no exception, returned ' . $f->type();
} catch (ErrorException $e) {
    $first = $e->getMessage();
}
$t->assertMatch('the first call throws because detection is refused', $first, '/open_basedir restriction/');

$t->assertThrows('the second call reads the file again and throws too', fn () => $f->type(), ErrorException::class);

$warnings = 0;
set_error_handler(static function () use (&$warnings): bool {
    $warnings++;
    return true;
}, E_WARNING);
$failed = $f->type();
$warningsAfterFailure = $warnings;
$kept = $f->type();
restore_error_handler();

$t->assertSame('without a throw the detection falls back', $failed, 'application/octet-stream');
$t->assertTrue('that detection warned', $warningsAfterFailure > 0);
$t->assertSame('the next call returns the kept answer', $kept, 'application/octet-stream');
$t->assertSame('and does not read the file again', $warnings, $warningsAfterFailure);

rmdir($dir);

$t->done();
