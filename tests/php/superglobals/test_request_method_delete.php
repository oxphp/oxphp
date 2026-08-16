<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_delete', 'superglobals');
$t->assertEqual('REQUEST_METHOD is DELETE', $_SERVER['REQUEST_METHOD'], 'DELETE');
$t->done();
