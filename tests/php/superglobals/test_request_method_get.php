<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_get', 'superglobals');
$t->assertEqual('REQUEST_METHOD is GET', $_SERVER['REQUEST_METHOD'], 'GET');
$t->done();
