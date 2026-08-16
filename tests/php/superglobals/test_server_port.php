<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('server_port', 'superglobals');
$t->assertGreaterThan('SERVER_PORT > 0', (int)$_SERVER['SERVER_PORT'], 0);
$t->done();
