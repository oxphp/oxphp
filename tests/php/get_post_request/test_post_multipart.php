<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('post_multipart', 'get_post_request');
$t->assertSame('$_POST[name] is "dio"', $_POST['name'], 'dio');
$t->done();
