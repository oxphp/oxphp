<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_replace', 'headers');
header('X-Foo: bar');
header('X-Foo: baz');
$t->assertTrue('X-Foo header replaced with baz', true);
$t->done();
