<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Asks the worker to leave once this request is answered. The exit happens
// after the response, so all this request can assert is that it was accepted.

$test = new TestCase('recycle_schedule', 'recycle');

$worker = OxPHP\Server\Worker::current();
$test->assertFalse('no exit is scheduled yet', $worker->isExitScheduled());

$worker->scheduleExit();

$test->assertTrue('the exit is scheduled', $worker->isExitScheduled());
$test->assertSame('the reason names the caller', $worker->exitReason(), 'scheduled');

$test->done();
