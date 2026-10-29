<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../async/orphan_child_probe.php';

$t = new TestCase('task_orphan_on_another_thread_outlives_the_budget', 'asyncfilecache');

// A task that ends without awaiting a task it dispatched cancels that task, and
// with no script in OPcache's shared memory the parent's result waits until the
// child has stopped, however long past the drain budget that is: the child's
// code is part of the parent's, in the memory of the request that awaits the
// parent, and that request ends as soon as it has the result.
//
// The code comes from OPcache's file cache, which keeps it in the memory of the
// request that loaded it. On the pool thread that memory is not this thread's
// own, so only what the request knew of the parent's code tells the parent that
// its child's code is in request memory.
//
// Two pool threads, both tasks on the CPU (fixture_orphan_child_at_parent_end.php):
// the parent never gives up its thread while it waits, so the child runs on the
// other one and only an interrupt reaches it. The cancelled child keeps working
// for six more seconds, past the five-second budget. So:
//
//   * the child was cancelled when its parent ended (`cancelled_at - parent_done
//     < 1`), and did run past the budget (`ended_at - cancelled_at > 5.5`);
//   * the response came only after the child had stopped (`ended_at < resp_at`);
//   * the server is still serving afterwards.
$t->assertSame('OPcache is on', (bool) ini_get('opcache.enable'), true);
$t->assertSame('OPcache keeps scripts in its file cache only', (bool) ini_get('opcache.file_cache_only'), true);

$linger = 6.0;
$run = oxphp_orphan_child_run($t, 'cpu', $linger, $linger + 3.0);

$t->assertContains('the parent returned once its child was running', $run['resp'], 'outer-done');
$t->assertGreaterThan('the child was cancelled', $run['cancelled_at'], 0.0);
$t->assertLessThan(
    'the child was cancelled when its parent ended',
    $run['cancelled_at'] - $run['parent_done'],
    1.0,
);
$t->assertGreaterThan('the child recorded a finish time', $run['ended_at'], 0.0);
$t->assertGreaterThan(
    'the child ran past the drain budget after its cancellation',
    $run['ended_at'] - $run['cancelled_at'],
    5.5,
);
$t->assertLessThan('the response waited for the child to stop', $run['ended_at'], $run['resp_at']);

$after = @file_get_contents(
    'http://127.0.0.1/tests/asyncnocache/fixture_lookup_array_object.php',
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);
$t->assertSame('the server still runs tasks afterwards', $after, 'ArrayObject');

$t->done();
