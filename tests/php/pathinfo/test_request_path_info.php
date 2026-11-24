<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_path_info', 'pathinfo');
// Runner hits: /tests/pathinfo/test_request_path_info.php/u%20ser/x
// The request methods carry what $_SERVER carries for a split path: the script
// part in scriptName(), the decoded tail in pathInfo(), PHP_SELF as the two
// joined.
$req = oxphp_http_request();
$t->assertSame('pathInfo() is the decoded tail', $req->pathInfo(), '/u ser/x');
$t->assertSame('pathInfo() === PATH_INFO', $req->pathInfo(), $_SERVER['PATH_INFO'] ?? null);
$t->assertSame('scriptName() is the script part', $req->scriptName(), '/tests/pathinfo/test_request_path_info.php');
$t->assertSame('scriptName() === SCRIPT_NAME', $req->scriptName(), $_SERVER['SCRIPT_NAME'] ?? null);
$t->assertSame('scriptFilename() === SCRIPT_FILENAME', $req->scriptFilename(), $_SERVER['SCRIPT_FILENAME'] ?? null);
$t->assertSame(
    'scriptName() . pathInfo() === PHP_SELF',
    $req->scriptName() . ($req->pathInfo() ?? ''),
    $_SERVER['PHP_SELF'] ?? null
);
$t->done();
