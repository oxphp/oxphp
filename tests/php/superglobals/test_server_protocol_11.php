<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('server_protocol_11', 'superglobals');
$t->assertMatch('SERVER_PROTOCOL matches HTTP/1 or HTTP/2', $_SERVER['SERVER_PROTOCOL'], '/^HTTP\/[12]/');
$t->done();
