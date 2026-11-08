<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('long_task_does_not_stop_the_pool', 'opcacherestart');

// A thread due for replacement stops taking tasks so that it reaches a moment
// with none, but one thread at a time, and for a bounded time. With each pool
// thread parked on a task that outlasts that time, the other thread takes the
// tasks that arrive while the first one waits, and the first one then takes
// tasks again. A thread that kept waiting would hold everything behind a task
// that may be waiting for one of those tasks itself.
$before = ox_opcache_state();
$t->assertSame('premise: no restart is pending', $before['restart_pending'], false);
$t->assertTrue('premise: both pool threads run with the cache', ox_poll_tasks(true, 10.0));

$base = sys_get_temp_dir() . '/oxphp_opcacherestart_long';
foreach (glob("$base.*") ?: [] as $f) {
    unlink($f);
}

$long = static function () use ($base): int {
    // Busy, so that the second task lands on the other thread.
    $until = microtime(true) + 0.1;
    while (microtime(true) < $until) {
    }
    $id = oxphp_worker_id();
    file_put_contents("$base.$id", '1');
    oxphp_sleep(8.0);
    return $id;
};
$a = oxphp_async($long);
$b = oxphp_async($long);

$deadline = microtime(true) + 3.0;
do {
    clearstatcache();
    $threads = glob("$base.*") ?: [];
    usleep(20000);
} while (count($threads) < 2 && microtime(true) < $deadline);
$t->assertSame('premise: a long task is parked on each pool thread', count($threads), 2);

$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);
$resetAt = microtime(true);

$quick = static function (): int {
    return oxphp_worker_id();
};

// While one thread waits for its task to end, the other still takes tasks.
usleep(1000000);
$start = microtime(true);
$first = oxphp_async_await(oxphp_async($quick), 4.0);
$elapsed = microtime(true) - $start;
$t->assertTrue('one thread still takes tasks while the other waits', $elapsed < 1.0);
$t->meta('first_task_seconds', round($elapsed, 3));

// Past the 5 s the first thread waits for, it takes tasks again, and the other
// thread is the one waiting now.
usleep((int) max(0, ($resetAt + 5.7 - microtime(true)) * 1000000));
$start = microtime(true);
$second = oxphp_async_await(oxphp_async($quick), 4.0);
$elapsed = microtime(true) - $start;
$t->assertTrue('a task is still taken after the wait has run out', $elapsed < 1.0);
$t->assertNotEqual('the thread that waited took it', $second, $first);
$t->meta('second_task_seconds', round($elapsed, 3));

oxphp_async_await_all([$a, $b], 10.0);

$t->done();
