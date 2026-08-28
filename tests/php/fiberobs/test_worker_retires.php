<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Asks the worker to retire once this request is answered.
//
// Retiring is what releases the request fibers, and releasing a fiber resumes
// it so its loop unwinds and returns. The engine notifies its observers on the
// switch away from a fiber that has returned, and that notification is the one
// place a running worker walks the whole chain of open calls looking for
// handlers a fatal left there. A chain that names itself is walked forever.
//
// This request is answered before any of that — the retire happens after the
// response — so what it asserts is only that the retire was accepted. The
// request after it is the one that needs a pool with a worker in it.

$t = new TestCase('worker_retires', 'fiberobs');

$worker = OxPHP\Server\Worker::current();
$before = $worker->requestCount();

$t->assertGreaterThan('this worker has served the requests before it', $before, 3);
$t->assertFalse('no exit is scheduled yet', $worker->isExitScheduled());

$worker->scheduleExit();

$t->assertTrue('the exit is scheduled', $worker->isExitScheduled());
$t->assertSame('the reason names the caller', $worker->exitReason(), 'scheduled');

$t->done();
