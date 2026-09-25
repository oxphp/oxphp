<?php
// Same key in the query, the body and a cookie. No php.ini is active in the
// images, so request_order is unset and PHP merges $_REQUEST by
// variables_order ("EGPCS"): GET, then POST, then COOKIE, each overwriting
// the previous one — the cookie wins.
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_cookie_order', 'get_post_request');
$t->assertSame('request_order is unset', ini_get('request_order'), '');
$t->assertSame('variables_order is "EGPCS"', ini_get('variables_order'), 'EGPCS');
$t->assertSame('$_GET[k] is "get"', $_GET['k'] ?? null, 'get');
$t->assertSame('$_POST[k] is "body"', $_POST['k'] ?? null, 'body');
$t->assertSame('$_COOKIE[k] is "cookie"', $_COOKIE['k'] ?? null, 'cookie');
$t->assertSame('$_REQUEST[k] is "cookie"', $_REQUEST['k'] ?? null, 'cookie');
$t->done();
