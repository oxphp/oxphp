<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// Two idle slots when the pool goes away on this worker thread: one minted
// here, one minted by a task on its own thread. This thread frees its own,
// through $destroy. The task's slot lives in the task thread's heap, which
// this thread must not free into: it is left there, without $destroy, and
// counted as skipped. Freeing it here aborted the process. A pool without a
// $destroy skips nothing, so leaving its slot there counts nothing.
$t = new TestCase('test_pool_idle_slot_of_a_task_outlives_the_pool', 'asyncdecl');

$skipped = static function (): ?int {
    $metrics = @file_get_contents('http://127.0.0.1:9090/metrics');
    if ($metrics === false || !preg_match(
        '/^oxphp_shared_pool_destroy_skipped_total\{reason="other_thread"\} (\d+)$/m',
        $metrics,
        $m,
    )) {
        return null;
    }
    return (int) $m[1];
};

$destroyed = 0;
$pool = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static function (object $r) use (&$destroyed): void {
        $destroyed++;
    },
    maxSize: 2,
);
$pool->acquire()->release();
[$ok] = asyncdecl_settle(oxphp_async(static function () use ($pool): bool {
    $pool->acquire()->release();
    return true;
}));
$t->assertTrue('the task returns', $ok);
$t->assertSame('both slots are idle', $pool->stats()->idle(), 2);

$before = $skipped();
$t->assertTrue('the skip counter is exported', $before !== null);
unset($pool);

$t->assertSame('$destroy ran for the slot minted here, not for the task\'s', $destroyed, 1);
$t->assertSame('the task\'s slot is counted as left without $destroy', $skipped(), $before + 1);

$plain = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
[$ok] = asyncdecl_settle(oxphp_async(static function () use ($plain): bool {
    $plain->acquire()->release();
    return true;
}));
$t->assertTrue('the task returns again', $ok);
$t->assertSame('the task\'s slot is idle', $plain->stats()->idle(), 1);
unset($plain);
$t->assertSame('a slot of a pool without $destroy is not counted', $skipped(), $before + 1);

$t->done();
