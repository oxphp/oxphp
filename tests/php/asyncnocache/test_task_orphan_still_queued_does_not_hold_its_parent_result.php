<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_orphan_still_queued_does_not_hold_its_parent_result', 'asyncnocache');

// Without OPcache a task's result is held while a task it dispatched and did
// not await still runs its code (test_task_orphan_holds_its_parent_result_until_it_stops).
// A child still waiting in the queue when its parent ends runs none: the pool
// answers a cancelled task without starting it. So its parent's result must not
// wait for the child's turn in the queue — and what the child holds is released
// before that result goes: the child's closure is part of the parent's code, and
// releasing it writes to its prototype in the memory of the request awaiting the
// parent. The closure is bound to an object whose destructor records that release.
//
// One pool thread. The parent spins on it, so nothing else is taken off the
// queue meanwhile; during the spin this request queues a long task, and the
// parent then dispatches its child and returns. The queue is then the long task,
// then the child: the child's turn comes only once the long task has run.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$base = sys_get_temp_dir() . '/orphan_queued_' . bin2hex(random_bytes(6));
$child_ran = $base . '.child_ran';
$long_ended = $base . '.long_ended';
$released = $base . '.released';

$p = oxphp_async(static function () use ($child_ran, $released): array {
    require_once __DIR__ . '/orphan_queued_child_owner.php';
    $started = microtime(true);
    $until = $started + 0.3;
    while (microtime(true) < $until) {
    }
    oxphp_async((new OrphanQueuedChildOwner($released))->child($child_ran));
    $child_queued = microtime(true);

    return [$started, $child_queued, microtime(true)];
});
usleep(50_000);
$l = oxphp_async(static function () use ($long_ended): float {
    $started = microtime(true);
    $until = $started + 2.0;
    while (microtime(true) < $until) {
    }
    file_put_contents($long_ended, (string) microtime(true));

    return $started;
});
$long_queued = microtime(true);

[$parent_started, $child_queued, $parent_done] = oxphp_async_await($p, 10.0);
$parent_at = microtime(true);
$long_started = oxphp_async_await($l, 10.0);
usleep(300_000);
$long_ended_at = is_file($long_ended) ? (float) file_get_contents($long_ended) : 0.0;
$ran = is_file($child_ran);
$released_at = is_file($released) ? (float) file_get_contents($released) : 0.0;
foreach ([$child_ran, $long_ended, $released] as $marker) {
    if (is_file($marker)) {
        unlink($marker);
    }
}

$t->meta('parent_elapsed', round($parent_at - $parent_done, 3));

$t->assertLessThan('the parent held the thread when the long task was queued', $parent_started, $long_queued);
$t->assertLessThan('the long task was queued ahead of the child', $long_queued, $child_queued);
$t->assertGreaterThan('the long task started after the parent ended', $long_started, $parent_done);
$t->assertGreaterThan('the long task recorded its end', $long_ended_at, 0.0);
$t->assertLessThan('the parent result did not wait for the long task', $parent_at, $long_ended_at);
$t->assertFalse('the child never ran', $ran);
$t->assertGreaterThan('the child closure was released', $released_at, 0.0);
$t->assertLessThan('the child closure was released before the parent result arrived', $released_at, $parent_at);

$t->done();
