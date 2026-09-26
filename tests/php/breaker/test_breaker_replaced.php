<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The request after a retire, answered by the replacement. It says only that the
// pool is serving again on a freshly booted worker; why the old one went is read
// from the server log by the suite lines that follow it.

$test = new TestCase('breaker_replaced', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$test->done();
