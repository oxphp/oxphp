<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An upload field left empty has no file to read, so type() falls back without
// looking for mime_content_type() and without a warning, even where the
// function is missing. Sent via curl as -F "avatar=@/dev/null;filename=", which
// produces an UPLOAD_ERR_NO_FILE entry.
$t = new TestCase('object_file_nofile_without_fileinfo', 'fileinfo_off');

$t->assertFalse('mime_content_type() is not available in this profile', function_exists('mime_content_type'));

$f = oxphp_http_request()->file('avatar');
$t->assertSame('error() is UPLOAD_ERR_NO_FILE', $f?->error(), UPLOAD_ERR_NO_FILE);

// The TestCase error handler turns any warning into an exception.
$t->assertSame('type() falls back without a warning', $f?->type(), 'application/octet-stream');

$t->done();
