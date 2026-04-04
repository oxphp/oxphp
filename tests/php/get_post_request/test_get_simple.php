<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('get_simple', 'get_post_request');
$t->assertSame('$_GET[a] is "1"', $_GET['a'], '1');
$t->assertSame('$_GET[b] is "2"', $_GET['b'], '2');
$t->done();
