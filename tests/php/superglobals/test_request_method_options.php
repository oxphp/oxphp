<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_options', 'superglobals');
$t->assertEqual('REQUEST_METHOD is OPTIONS', $_SERVER['REQUEST_METHOD'], 'OPTIONS');
$t->done();
