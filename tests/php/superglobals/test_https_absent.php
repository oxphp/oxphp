<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('https_absent', 'superglobals');
$t->assertKeyMissing('HTTPS key is missing', $_SERVER, 'HTTPS');
$t->done();
