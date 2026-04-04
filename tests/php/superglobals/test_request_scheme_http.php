<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_scheme_http', 'superglobals');
$t->assertEqual('REQUEST_SCHEME is http', $_SERVER['REQUEST_SCHEME'], 'http');
$t->done();
