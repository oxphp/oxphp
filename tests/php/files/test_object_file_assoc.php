<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Associative array file field: name="docs[passport]" / name="docs[license]".
// PHP keys the parallel $_FILES sub-arrays (name/tmp_name/error/size) by the
// string subscript, so files('docs') must pair each tmp_name slot with the
// same-keyed name/error/size slot — every entry stays a valid, named upload.
$t = new TestCase('object_file_assoc', 'files');

$req = oxphp_http_request();

$docs = $req->files('docs');
$t->assertCount('files(docs) lists both associative entries', $docs, 2);

foreach ($docs as $i => $doc) {
    $t->assertInstanceOf("entry $i is UploadedFileInterface", $doc, 'OxPHP\Http\UploadedFileInterface');
    $t->assertTrue("entry $i isValid()", $doc->isValid());
    $t->assertSame("entry $i error() is UPLOAD_ERR_OK", $doc->error(), UPLOAD_ERR_OK);
    $t->assertSame("entry $i name() is the client filename", $doc->name(), 'small.txt');
    $t->assertSame("entry $i size() is the byte length", $doc->size(), 11);
    $t->assertTrue("entry $i tmpPath() points at a real uploaded file", is_uploaded_file($doc->tmpPath()));
}

$t->done();
