<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The text half and the file half of one multipart body, read through the
// object API alone. They come from the same parse, so a flag that hides one
// from payload() while file() still answers is the object API disagreeing
// with itself about a single request.
$t = new TestCase('payload_multipart', 'superglobals_off');

$req = oxphp_http_request();

$t->assertSame('payload("field") === "fieldval"', $req->payload('field'), 'fieldval');

$file = $req->file('doc');
$t->assertNotNull('file("doc") is present', $file);
$t->assertSame('file("doc")->name()', $file?->name(), 'small.txt');
$t->assertCount('files() lists one file', $req->files(), 1);

$t->done();
