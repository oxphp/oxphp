<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('document_root', 'superglobals');
$t->assertNotEmpty('DOCUMENT_ROOT is not empty', $_SERVER['DOCUMENT_ROOT']);
$t->assertTrue('DOCUMENT_ROOT is a directory', is_dir($_SERVER['DOCUMENT_ROOT']));
$t->done();
