<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('body_not_php', 'ratelimit');
// The runner verifies that the 429 response body is not PHP output.
// This script is a placeholder; actual assertion is done by the runner.
$t->assertTrue('placeholder — runner checks 429 body is not PHP', true);
$t->done();
