<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('retry_after_header', 'overflow');
// This script intentionally blocks the worker for 10 seconds.
// The runner sends concurrent requests to saturate the queue and checks that
// the 529 response includes a Retry-After header.
sleep(10);
$t->assertTrue('worker held for Retry-After header test', true);
$t->done();
