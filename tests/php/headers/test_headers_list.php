<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('headers_list', 'headers');
header('X-List-A: alpha');
header('X-List-B: beta');
$list = headers_list();
$joined = implode("\n", $list);
$t->assertNotEmpty('headers_list() is not empty', $list);
$t->assertContains('headers_list contains X-List-A: alpha', $joined, 'X-List-A: alpha');
$t->assertContains('headers_list contains X-List-B: beta', $joined, 'X-List-B: beta');
$t->done();
