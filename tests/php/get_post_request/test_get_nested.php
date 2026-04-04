<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('get_nested', 'get_post_request');
$t->assertSame('$_GET[user][name] is "dio"', $_GET['user']['name'], 'dio');
$t->assertSame('$_GET[user][age] is "30"', $_GET['user']['age'], '30');
$t->done();
