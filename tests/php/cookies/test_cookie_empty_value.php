<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_empty_value', 'cookies');
$t->assertKeyExists('$_COOKIE[key] exists', $_COOKIE, 'key');
$t->assertSame('$_COOKIE[key] is ""', $_COOKIE['key'], '');
$t->done();
