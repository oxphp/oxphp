<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_single', 'files');

$t->assertNotEmpty('$_FILES is not empty', $_FILES);

$file = reset($_FILES);
$t->assertKeyExists('has name key', $file, 'name');
$t->assertKeyExists('has type key', $file, 'type');
$t->assertKeyExists('has tmp_name key', $file, 'tmp_name');
$t->assertKeyExists('has error key', $file, 'error');
$t->assertKeyExists('has size key', $file, 'size');

$t->assertSame('error is 0', $file['error'], UPLOAD_ERR_OK);
$t->assertGreaterThan('size > 0', $file['size'], 0);
$t->assertTrue('is_uploaded_file returns true', is_uploaded_file($file['tmp_name']));

$t->done();
