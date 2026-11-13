<?php

declare(strict_types=1);

// The same for oxphp_async() tasks: a fatal in one task leaves the objects a
// task parked beside it holds their destructors. This profile runs one async
// worker, so both tasks run on the same thread and share its objects.

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_fatal_keeps_a_parked_tasks_destructors', 'fibers');

$failedAt = sys_get_temp_dir() . '/oxphp-destruct-probe-task-failed-at';
if (is_file($failedAt)) {
    unlink($failedAt);
}

$parked = oxphp_async(static function (): array {
    require_once __DIR__ . '/destruct_probe.php';
    OxphpDestructProbe::reset(['task-parked', 'task-failed']);
    $held = new OxphpDestructProbe('task-parked');
    $from = microtime(true);
    sleep(1);
    $until = microtime(true);
    unset($held);

    return [$from, $until, OxphpDestructProbe::destructed('task-parked'), OxphpDestructProbe::destructed('task-failed')];
});

$failing = oxphp_async(static function (string $failedAt): void {
    require_once __DIR__ . '/destruct_probe.php';
    $own = new OxphpDestructProbe('task-failed');
    usleep(200_000);
    file_put_contents($failedAt, (string) microtime(true));
    oxphp_destruct_probe_fatal();
}, $failedAt);

try {
    oxphp_async_await($failing, 10.0);
    $outcome = 'returned';
} catch (\Throwable $e) {
    $outcome = 'failed';
}
$t->assertSame('the failing task failed', $outcome, 'failed');

[$from, $until, $parkedRan, $failedRan] = oxphp_async_await($parked, 10.0);
$at = is_file($failedAt) ? (float) file_get_contents($failedAt) : 0.0;
$t->assertTrue('the other task failed while this one was parked', $at > $from && $at < $until);
$t->assertTrue("the destructor of the parked task's object ran", $parkedRan);
$t->assertFalse("the failed task's object had no destructor run", $failedRan);

$t->done();
