<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('remote_port', 'superglobals');
$t->assertGreaterThan('REMOTE_PORT > 0', (int)$_SERVER['REMOTE_PORT'], 0);
$t->done();
