<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_uri', 'superglobals');
$t->assertKeyExists('REQUEST_URI key exists', $_SERVER, 'REQUEST_URI');
$t->assertContains('REQUEST_URI contains filename', $_SERVER['REQUEST_URI'], 'test_request_uri.php');
$t->done();
