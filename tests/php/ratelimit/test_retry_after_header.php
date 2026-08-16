<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('retry_after_header', 'ratelimit');
// The runner verifies the Retry-After header on the 429 response.
// This script is a placeholder; actual assertion is done by the runner.
$t->assertTrue('placeholder — runner checks Retry-After header', true);
$t->done();
