<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('content_length', 'superglobals');
$t->assertGreaterThan('CONTENT_LENGTH > 0', (int)$_SERVER['CONTENT_LENGTH'], 0);
$t->done();
