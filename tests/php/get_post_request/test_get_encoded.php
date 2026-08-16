<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('get_encoded', 'get_post_request');
$t->assertSame('$_GET[q] is "hello world"', $_GET['q'], 'hello world');
$t->done();
