<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../async/orphan_child_probe.php';

$t = new TestCase('task_orphan_holds_its_parent_result_until_it_stops', 'asyncnocache');

// A task that ends without awaiting a task it dispatched cancels that task.
// Without OPcache the child's code is part of the parent's, which lives in the
// memory of the request that dispatched the parent, and that memory is released
// when the request ends — which it does as soon as it has the parent's result.
// So the parent's result must not reach the request while the child still runs.
//
// One pool thread: the parent and the child take turns on it, both parked in
// oxphp_sleep() (fixture_orphan_child_at_parent_end.php). The cancelled child
// keeps working for two more seconds. So:
//
//   * the child was cancelled when its parent ended, not left to run out its
//     20 s (`cancelled_at - parent_done < 1`);
//   * the response came only after the child had stopped (`ended_at < resp_at`),
//     and the child did get its two seconds (`ended_at - cancelled_at`): a
//     response that comes first would end the request under a child still
//     running its code;
//   * the server is still serving afterwards.
//
// With OPcache's shared memory the parent's result is not held —
// `async/test_task_orphan_cancelled_when_its_parent_ends`.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$linger = 2.0;
$run = oxphp_orphan_child_run($t, 'park', $linger, $linger + 3.0);

$t->assertContains('the parent returned once its child was running', $run['resp'], 'outer-done');
$t->assertGreaterThan('the child was cancelled', $run['cancelled_at'], 0.0);
$t->assertLessThan(
    'the child was cancelled when its parent ended',
    $run['cancelled_at'] - $run['parent_done'],
    1.0,
);
$t->assertGreaterThan('the child recorded a finish time', $run['ended_at'], 0.0);
$t->assertGreaterThan(
    'the child kept working after its cancellation',
    $run['ended_at'] - $run['cancelled_at'],
    $linger - 0.1,
);
$t->assertLessThan('the response waited for the child to stop', $run['ended_at'], $run['resp_at']);

$after = @file_get_contents(
    'http://127.0.0.1/tests/asyncnocache/fixture_lookup_array_object.php',
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);
$t->assertSame('the server still runs tasks afterwards', $after, 'ArrayObject');

$t->done();
