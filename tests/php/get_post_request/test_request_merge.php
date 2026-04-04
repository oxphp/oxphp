<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_merge', 'get_post_request');
$t->assertSame('$_REQUEST[get_key] is "get_val"', $_REQUEST['get_key'], 'get_val');
$t->assertSame('$_REQUEST[post_key] is "post_val"', $_REQUEST['post_key'], 'post_val');
$t->done();
