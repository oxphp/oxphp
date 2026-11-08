<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A form field named with a canonical integer lands in $_FILES under an integer
// key, like any other variable PHP registers.
$t = new TestCase('file_numeric_field', 'http_object');

$req = oxphp_http_request();

$t->assertKeyExists('$_FILES holds the field under an integer key', $_FILES, 0);
$t->assertSame('file("0")->name()', $req->file('0')?->name(), 'small.txt');
$t->assertCount('files("12") lists the file', $req->files('12'), 1);
$t->assertNotNull('file("named") is found', $req->file('named'));
$t->assertNull('an absent numeric field is null', $req->file('99'));

$t->done();
