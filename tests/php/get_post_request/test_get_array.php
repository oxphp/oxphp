<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('get_array', 'get_post_request');
$t->assertSame('$_GET[tags] is array ["php","rust"]', $_GET['tags'], ['php', 'rust']);
$t->done();
