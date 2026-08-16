<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_put', 'superglobals');
$t->assertEqual('REQUEST_METHOD is PUT', $_SERVER['REQUEST_METHOD'], 'PUT');
$t->done();
