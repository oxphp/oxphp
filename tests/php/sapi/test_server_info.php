<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('server_info', 'sapi');

$info = oxphp_server_info();
$t->assertType('oxphp_server_info() is array', $info, 'array');
$t->assertKeyExists('has key: version', $info, 'version');
$t->assertKeyExists('has key: worker_id', $info, 'worker_id');
$t->assertKeyExists('has key: request_time', $info, 'request_time');
$t->assertKeyExists('has key: worker_mode', $info, 'worker_mode');

// SERVER_SOFTWARE carries the Rust binary's version; the extension must report the same one.
$server = $_SERVER['SERVER_SOFTWARE'] ?? '';
$t->assertSame('version is the server version', 'OxPHP/' . $info['version'], $server);
$t->assertSame('phpversion(oxphp_sapi) is the server version', 'OxPHP/' . phpversion('oxphp_sapi'), $server);

$t->done();
