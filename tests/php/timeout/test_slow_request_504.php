<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('slow_request_504', 'timeout');
// This script intentionally sleeps longer than the server timeout.
// The server will kill the worker and return 504 before this assertion runs.
// The runner checks for the 504 status code — not the JSON output of this script.
sleep(5);
$t->assertTrue('should never reach here — server should return 504', true);
$t->done();
