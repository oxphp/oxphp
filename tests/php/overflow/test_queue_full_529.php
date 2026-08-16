<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('queue_full_529', 'overflow');
// This script intentionally blocks the worker for 10 seconds.
// The runner sends concurrent requests to saturate the queue; subsequent requests
// should receive HTTP 529 (queue full) while this script holds a worker.
sleep(10);
$t->assertTrue('worker held for queue saturation test', true);
$t->done();
