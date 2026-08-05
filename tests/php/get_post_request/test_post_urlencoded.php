<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('post_urlencoded', 'get_post_request');
$t->assertSame('$_POST[name] is "dio"', $_POST['name'], 'dio');
$t->assertSame('$_POST[lang] is "php"', $_POST['lang'], 'php');
$t->done();
