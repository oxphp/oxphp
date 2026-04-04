<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_method_post', 'superglobals');
$t->assertEqual('REQUEST_METHOD is POST', $_SERVER['REQUEST_METHOD'], 'POST');
$t->done();
