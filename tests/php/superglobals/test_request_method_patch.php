<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_patch', 'superglobals');
$t->assertEqual('REQUEST_METHOD is PATCH', $_SERVER['REQUEST_METHOD'], 'PATCH');
$t->done();
