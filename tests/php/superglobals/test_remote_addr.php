<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('remote_addr', 'superglobals');
$t->assertNotEmpty('REMOTE_ADDR is not empty', $_SERVER['REMOTE_ADDR']);
$t->assertMatch('REMOTE_ADDR is valid IP', $_SERVER['REMOTE_ADDR'], '/^[\d.:a-fA-F]+$/');
$t->done();
