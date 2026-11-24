<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// With SUPERGLOBALS_ENABLED=false the request keys of $_SERVER are gone, and
// these methods are the only place they can be read. The values are written
// out rather than compared with $_SERVER, which has nothing to compare with.
$t = new TestCase('request_script_params', 'superglobals_off');

$req = oxphp_http_request();

$t->assertKeyMissing('$_SERVER has no SCRIPT_NAME', $_SERVER, 'SCRIPT_NAME');
$t->assertKeyMissing('$_SERVER has no REMOTE_PORT', $_SERVER, 'REMOTE_PORT');

if (oxphp_is_worker()) {
    // The worker entry sits outside the document root, so its script name is
    // its basename, as in $_SERVER with the superglobals on.
    $t->assertSame('scriptName() is the worker entry', $req->scriptName(), '/worker_entry.php');
    $t->assertSame('scriptFilename() is the worker entry', $req->scriptFilename(), '/var/www/html/worker/worker_entry.php');
} else {
    $t->assertSame('scriptName() names this file', $req->scriptName(), '/tests/superglobals_off/test_request_script_params.php');
    $t->assertSame('scriptFilename() is this file', $req->scriptFilename(), __FILE__);
}
$t->assertNull('pathInfo() is null without PATH_INFO', $req->pathInfo());
$t->assertGreaterThan('remotePort() is a client port', $req->remotePort(), 0);
$t->assertNull('deniedPattern() is null outside a deny fallback', $req->deniedPattern());
$t->assertNull('traceContext() is null with tracing off', $req->traceContext());
$t->assertSame('document_root is the configured root', oxphp_server_info()['document_root'] ?? null, '/var/www/html/public');

$t->done();
