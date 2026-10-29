<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/orphan_child_probe.php';

$t = new TestCase('task_orphan_cancelled_when_its_parent_ends', 'async');

// A task that ends without awaiting a task it dispatched cancels that task, as
// the end of a request cancels the tasks the request did not await. Here the
// code is in OPcache's shared memory, which the end of the request does not
// release, so nothing makes the parent's result wait for the child: the request
// gets it at once, and the child finishes its cancellation on its own.
//
// The child is parked in oxphp_sleep() (fixture_orphan_child_at_parent_end.php)
// and keeps working for three seconds once cancelled. So:
//
//   * the child was cancelled when its parent ended, not left to run out its
//     20 s (`cancelled_at - parent_done < 1`);
//   * the response did not wait for it: it came before the child stopped and
//     sooner than the three seconds the child still had to run.
//
// Without OPcache the response waits —
// `asyncnocache/test_task_orphan_holds_its_parent_result_until_it_stops`.
$t->assertSame('OPcache is on', (bool) ini_get('opcache.enable'), true);
$t->assertSame('OPcache keeps scripts in shared memory', (bool) ini_get('opcache.file_cache_only'), false);

$linger = 3.0;
$run = oxphp_orphan_child_run($t, 'park', $linger, $linger + 3.0);

$t->assertContains('the parent returned once its child was running', $run['resp'], 'outer-done');
$t->assertGreaterThan('the child was cancelled', $run['cancelled_at'], 0.0);
$t->assertLessThan(
    'the child was cancelled when its parent ended',
    $run['cancelled_at'] - $run['parent_done'],
    1.0,
);
$t->assertGreaterThan('the child recorded a finish time', $run['ended_at'], 0.0);
$t->assertGreaterThan('the response came before the child stopped', $run['ended_at'], $run['resp_at']);
$t->assertLessThan('the response did not wait for the child', $run['resp_at'] - $run['t0'], $linger);

$t->done();
