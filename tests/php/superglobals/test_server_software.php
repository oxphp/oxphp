<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('server_software', 'superglobals');
$t->assertContains('SERVER_SOFTWARE contains OxPHP/', $_SERVER['SERVER_SOFTWARE'], 'OxPHP/');
$t->done();
