<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('get_no_value', 'get_post_request');
$t->assertKeyExists('$_GET[key] exists', $_GET, 'key');
$t->assertSame('$_GET[key] is ""', $_GET['key'], '');
$t->done();
