<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_replace', 'headers');
header('X-Foo: bar');
header('X-Foo: baz');
$joined = implode("\n", headers_list());
$t->assertContains('headers_list contains X-Foo: baz', $joined, 'X-Foo: baz');
$t->assertNotContains('headers_list drops earlier X-Foo: bar', $joined, 'X-Foo: bar');
$t->done();
