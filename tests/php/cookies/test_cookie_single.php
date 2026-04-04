<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_single', 'cookies');
$t->assertSame('$_COOKIE[foo] is "bar"', $_COOKIE['foo'], 'bar');
$t->done();
