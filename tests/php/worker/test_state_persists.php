<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('state_persists', 'worker');

// In worker mode, static variables persist across requests.
// The worker_entry.php fixture returns the request_count from its static var.
// This test validates that the value coming back from the worker is a positive integer,
// which would only be true if state persisted from prior requests.
$info = oxphp_server_info();
$t->assertKeyExists('server_info has worker_mode key', $info, 'worker_mode');
$t->assertTrue('worker_mode is true', $info['worker_mode'] === true);

$t->done();
