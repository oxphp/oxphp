<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('under_limit_ok', 'ratelimit');
// If PHP executes this script, the request was accepted (under the rate limit).
$t->assertTrue('request accepted under rate limit', true);
$t->done();
