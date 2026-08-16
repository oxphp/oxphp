<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('content_type', 'superglobals');
$t->assertEqual('CONTENT_TYPE is application/json', $_SERVER['CONTENT_TYPE'], 'application/json');
$t->done();
