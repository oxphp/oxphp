<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('upload_array', 'files');

$t->assertKeyExists('$_FILES has files key', $_FILES, 'files');

if (array_key_exists('files', $_FILES)) {
    $files = $_FILES['files'];
    $t->assertKeyExists('files has name key', $files, 'name');
    $t->assertKeyExists('files has type key', $files, 'type');
    $t->assertKeyExists('files has tmp_name key', $files, 'tmp_name');
    $t->assertKeyExists('files has error key', $files, 'error');
    $t->assertKeyExists('files has size key', $files, 'size');
    $t->assertType('files[name] is array', $files['name'], 'array');
    $t->assertType('files[error] is array', $files['error'], 'array');
}

$t->done();
