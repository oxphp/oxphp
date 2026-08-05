<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that suspends must come back still owning its own uploads.
//
// PHP records which paths are uploads of the request asking in one thread-wide
// slot, SG(rfc1867_uploaded_files), and that is the only thing
// is_uploaded_file() and move_uploaded_file() consult — the path alone tells
// them nothing. A worker serving several requests at once resets that slot for
// every request it accepts, so a fiber parked in a hooked sleep used to resume
// with its uploads no longer recognised as uploads: is_uploaded_file() false and
// move_uploaded_file() refusing, for a file that is sitting right there on disk.
// The temp file is then left behind for good, because the request that owns it
// no longer has anything to release.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is suspended. It uploads a file of its own, which is what leaves an
// intruder's uploaded-file state standing where this request will look.

$t = new TestCase('files_survive_suspend', 'hooks');

$file = $_FILES['doc'] ?? null;
$t->assertTrue('this request received its upload', is_array($file));

$tmp = is_array($file) ? (string) ($file['tmp_name'] ?? '') : '';
$name = is_array($file) ? (string) ($file['name'] ?? '') : '';

$t->assertSame('upload error is UPLOAD_ERR_OK', is_array($file) ? $file['error'] ?? null : null, UPLOAD_ERR_OK);
$t->assertTrue('is_uploaded_file() before the suspend', $tmp !== '' && is_uploaded_file($tmp));

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

$boundary = '----oxphpIntruderBoundary5d17';
$innerBody = "--{$boundary}\r\n"
    . "Content-Disposition: form-data; name=\"doc\"; filename=\"intruder.txt\"\r\n"
    . "Content-Type: text/plain\r\n\r\n"
    . "intruder-upload-9c02\r\n"
    . "--{$boundary}--\r\n";
fwrite($sock, "POST /tests/hooks/fixture_inner_upload.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "Content-Type: multipart/form-data; boundary={$boundary}\r\n"
    . 'Content-Length: ' . strlen($innerBody) . "\r\n"
    . "Connection: close\r\n\r\n"
    . $innerBody);

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without these two the rest proves nothing: if the intruder never ran inside
// the suspended window, or ran without uploading anything, this request's
// uploaded-file state was never at risk in the first place.
$t->assertContains('intruder was served while this request was parked', $resp, 'INNER-OK');
$t->assertContains('intruder uploaded its own file', $resp, 'intruder-upload-9c02');

// $_FILES itself travels with the request already, through PG(http_globals) —
// asserted here so a failure below can be read as being about the uploaded-file
// registry rather than about the superglobal.
$after = $_FILES['doc'] ?? null;
$t->assertSame('$_FILES still names this request\'s upload', is_array($after) ? $after['name'] ?? null : null, $name);
$t->assertSame('$_FILES still names this request\'s temp path', is_array($after) ? $after['tmp_name'] ?? null : null, $tmp);

// The discriminating checks. Both read SG(rfc1867_uploaded_files), and both
// answer "no" for a request whose registry entry was taken over by the intruder
// — for a file that is still on disk and still named by $_FILES.
$t->assertTrue('the temp file is still on disk after the suspend', $tmp !== '' && is_file($tmp));
$t->assertTrue('is_uploaded_file() after the suspend', $tmp !== '' && is_uploaded_file($tmp));

// Guarded rather than suppressed: TestCase installs an error handler that turns
// every warning into an ErrorException, and `@` does not stop a custom handler
// from being called — unlinking a path that is not there would end the test as a
// fatal instead of an assertion.
$dest = sys_get_temp_dir() . '/oxphp_files_survive_suspend_' . getmypid() . '.txt';
if (is_file($dest)) {
    unlink($dest);
}
$moved = $tmp !== '' && move_uploaded_file($tmp, $dest);
$t->assertTrue('move_uploaded_file() after the suspend', $moved);
if ($moved) {
    $t->assertSame('the moved file is this request\'s upload', file_get_contents($dest), 'hello world');
    unlink($dest);
}

$t->done();
