<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Follows three requests whose session write fatals. Had the worker read them
// as requests that completed, each would have cleared the run instead of adding
// to it, and this probe would be answered by the worker that served them.

$test = new TestCase('breaker_session_write_retires', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$test->done();
