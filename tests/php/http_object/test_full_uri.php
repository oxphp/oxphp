<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('full_uri', 'http_object');
$req = oxphp_http_request();
$t->assertContains('fullUri() contains "http"', $req->fullUri(), 'http');
$t->assertContains('fullUri() contains "test_full_uri.php"', $req->fullUri(), 'test_full_uri.php');
$t->done();
