<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('path', 'http_object');
$req = oxphp_http_request();
$t->assertContains('path() contains test_path.php', $req->path(), 'test_path.php');
$t->assertNotContains('path() does not contain ?', $req->path(), '?');
$t->done();
