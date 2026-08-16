<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('server_name_default', 'superglobals');
$t->assertKeyExists('SERVER_NAME key exists', $_SERVER, 'SERVER_NAME');
$t->assertNotEmpty('SERVER_NAME is not empty', $_SERVER['SERVER_NAME']);
$t->done();
