<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_no_replace', 'headers');
header('X-Multi: a', false);
header('X-Multi: b', false);
$headersList = implode("\n", headers_list());
$t->assertContains('headers_list contains X-Multi: a', $headersList, 'X-Multi: a');
$t->assertContains('headers_list contains X-Multi: b', $headersList, 'X-Multi: b');
$t->done();
