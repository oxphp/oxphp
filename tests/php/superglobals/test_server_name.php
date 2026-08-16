<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('server_name', 'superglobals');
$t->assertNotEmpty('SERVER_NAME is not empty', $_SERVER['SERVER_NAME']);
$t->done();
