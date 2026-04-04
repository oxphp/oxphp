<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('index_fallback', 'routing');
$t->assertKeyExists('SCRIPT_NAME key exists', $_SERVER, 'SCRIPT_NAME');
$t->assertNotEmpty('SCRIPT_NAME is not empty', $_SERVER['SCRIPT_NAME']);
$t->done();
