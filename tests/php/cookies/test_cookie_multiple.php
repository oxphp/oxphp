<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_multiple', 'cookies');
$t->assertSame('$_COOKIE[a] is "1"', $_COOKIE['a'], '1');
$t->assertSame('$_COOKIE[b] is "2"', $_COOKIE['b'], '2');
$t->done();
