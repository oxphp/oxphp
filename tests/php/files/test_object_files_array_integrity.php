<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('object_files_array_integrity', 'files');

$req = oxphp_http_request();

$photos = $req->files('photos');
$t->assertCount('files(photos) lists both files', $photos, 2);

foreach ($photos as $i => $photo) {
    $t->assertInstanceOf("entry $i is UploadedFileInterface", $photo, 'OxPHP\Http\UploadedFileInterface');
    $t->assertTrue("entry $i isValid()", $photo->isValid());
    $t->assertSame("entry $i error() is UPLOAD_ERR_OK", $photo->error(), UPLOAD_ERR_OK);
    $t->assertSame("entry $i name() is the client filename", $photo->name(), 'small.txt');
    $t->assertSame("entry $i size() is the byte length", $photo->size(), 11);
    $t->assertTrue("entry $i tmpPath() points at a real uploaded file", is_uploaded_file($photo->tmpPath()));
}

$first = $req->file('photos');
$t->assertInstanceOf('file(photos) returns the first file of the array field', $first, 'OxPHP\Http\UploadedFileInterface');
$t->assertTrue('file(photos) first is valid', $first->isValid());
$t->assertSame('file(photos) first name matches', $first->name(), 'small.txt');

$t->done();
