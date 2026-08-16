<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_special_chars', 'cookies');
$t->assertSame('$_COOKIE[encoded] is "hello world"', $_COOKIE['encoded'], 'hello world');
$t->done();
