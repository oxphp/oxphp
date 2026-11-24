<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_script_params', 'http_object');

// Every $_SERVER key the request methods have no other source for, against the
// methods that now carry it. Default profile: traditional routing, tracing off,
// no deny fallback.
$req = oxphp_http_request();

$t->assertSame('scriptName() === SCRIPT_NAME', $req->scriptName(), $_SERVER['SCRIPT_NAME'] ?? null);
$t->assertSame('scriptName() names this file', $req->scriptName(), '/tests/http_object/test_request_script_params.php');
$t->assertSame('scriptName() === DOCUMENT_URI', $req->scriptName(), $_SERVER['DOCUMENT_URI'] ?? null);
$t->assertSame('scriptFilename() === SCRIPT_FILENAME', $req->scriptFilename(), $_SERVER['SCRIPT_FILENAME'] ?? null);
$t->assertSame('scriptFilename() is this file', $req->scriptFilename(), __FILE__);
$t->assertNull('pathInfo() is null without PATH_INFO', $req->pathInfo());
$t->assertKeyMissing('$_SERVER has no PATH_INFO either', $_SERVER, 'PATH_INFO');
$t->assertSame(
    'scriptName() . pathInfo() === PHP_SELF',
    $req->scriptName() . ($req->pathInfo() ?? ''),
    $_SERVER['PHP_SELF'] ?? null
);
$t->assertSame('remotePort() === REMOTE_PORT', $req->remotePort(), (int) ($_SERVER['REMOTE_PORT'] ?? -1));
$t->assertGreaterThan('remotePort() is a client port', $req->remotePort(), 0);
$t->assertNull('deniedPattern() is null outside a deny fallback', $req->deniedPattern());
$t->assertNull('traceContext() is null with tracing off', $req->traceContext());
$t->assertKeyMissing('$_SERVER has no OXPHP_TRACE_ID either', $_SERVER, 'OXPHP_TRACE_ID');

$info = oxphp_server_info();
$t->assertKeyExists('oxphp_server_info() has document_root', $info, 'document_root');
$t->assertSame('document_root === DOCUMENT_ROOT', $info['document_root'], $_SERVER['DOCUMENT_ROOT'] ?? null);

$t->done();
