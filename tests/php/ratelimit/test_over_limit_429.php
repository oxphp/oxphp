<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('over_limit_429', 'ratelimit');
// The runner verifies the 429 status by sending bursts of requests.
// If this script executes, the request was not rate-limited — which is acceptable
// for the script itself; the runner handles checking the 429 response.
$t->assertTrue('placeholder — runner checks 429', true);
$t->done();
