<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('gateway_interface', 'superglobals');
$t->assertEqual('GATEWAY_INTERFACE is CGI/1.1', $_SERVER['GATEWAY_INTERFACE'], 'CGI/1.1');
$t->done();
