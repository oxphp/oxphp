<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_result_not_held_by_another_tasks_orphan', 'asyncfilecache');

// The result a task leaves while a task it did not await still runs is held
// back, but only that task's: another task that ends on the same thread while
// the orphan runs has its result delivered at once.
//
// fixture_orphan_child_at_parent_end.php in `cpu` mode: the parent spins on one
// pool thread, the child on the other, and the cancelled child keeps the second
// thread busy for four more seconds. A task dispatched in that window can only
// run on the first thread, the one that holds the parent's result. So:
//
//   * the task returns well before the orphan stops, and the orphan was still
//     running when it did (`ended_at` is later);
//   * the parent's result was still held, as asyncfilecache/
//     test_task_orphan_on_another_thread_outlives_the_budget proves on its own.
$t->assertSame('OPcache keeps scripts in its file cache only', (bool) ini_get('opcache.file_cache_only'), true);

$linger = 4.0;
$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_orphanchild_{$id}";

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 40);
fwrite($sock, "GET /tests/async/fixture_orphan_child_at_parent_end.php"
    . "?id={$id}&linger={$linger}&mode=cpu HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$deadline = microtime(true) + 6.0;
while (!file_exists("$base.cancelled_at") && microtime(true) < $deadline) {
    usleep(10000);
    clearstatcache();
}
$t->assertTrue('the orphan was cancelled', file_exists("$base.cancelled_at"));

$t0 = microtime(true);
$quick = oxphp_async_await(oxphp_async(static fn (): string => 'quick'), 10.0);
$quick_at = microtime(true);

$resp = (string) stream_get_contents($sock);
fclose($sock);
$resp_at = microtime(true);

$wait = $resp_at + 3.0;
while (!file_exists("$base.ended_at") && microtime(true) < $wait) {
    usleep(20000);
    clearstatcache();
}
$read = static fn (string $s): float
    => is_file("$base.$s") ? (float) file_get_contents("$base.$s") : 0.0;
$ended_at = $read('ended_at');
foreach (['started', 'parent_done', 'cancelled_at', 'ended_at'] as $s) {
    if (is_file("$base.$s")) {
        @unlink("$base.$s");
    }
}
$t->meta('quick_elapsed', round($quick_at - $t0, 3));

$t->assertSame('the task returned', $quick, 'quick');
$t->assertLessThan('the task was not held while the orphan ran', $quick_at - $t0, 1.0);
$t->assertGreaterThan('the orphan was still running when the task returned', $ended_at, $quick_at);
$t->assertContains('the parent returned once its child was running', $resp, 'outer-done');
$t->assertLessThan('the parent result waited for the orphan to stop', $ended_at, $resp_at);

$t->done();
