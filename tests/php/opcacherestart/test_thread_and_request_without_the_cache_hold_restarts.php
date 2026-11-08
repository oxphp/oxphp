<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('thread_and_request_without_the_cache_hold_restarts', 'opcacherestart');

// Second of three. The task that declared a function has ended, and only the
// holding task keeps the restart back. The thread that ran the declaring task
// is replaced all the same: the probes below, which the busy thread cannot
// take, stop finding the function. A server that lets that thread run on
// without holding the restart keeps it, and runs the restart under it once the
// holding task has ended.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_adopt';
$t->assertTrue('premise: the holding task is running', is_file("$base.started"));

$mine = ox_opcache_state();
$t->assertSame('premise: the restart is pending', $mine['restart_pending'], true);
$t->assertSame('premise: the restart has not run', $mine['manual_restarts'], 2);
$t->assertSame('premise: this request runs without the cache', $mine['opcache_enabled'], false);

$probe = static fn (): bool => function_exists('ox_opcacherestart_declared');
$deadline = microtime(true) + 5.0;
do {
    $left = oxphp_async_await(oxphp_async($probe), 4.0);
    if ($left === false) {
        break;
    }
    usleep(100000);
} while (microtime(true) < $deadline);
$t->assertSame('the thread that ran the declaring task was replaced while the restart was held', $left, false);

touch("$base.go");
$deadline = microtime(true) + 5.0;
while (!file_exists("$base.report") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$report = is_file("$base.report") ? json_decode((string) file_get_contents("$base.report"), true) : null;
$t->assertSame('the restart did not run under the holding task', $report['manual_restarts'] ?? null, 2);

$ran = ox_poll_state(static fn (array $s): bool => $s['manual_restarts'] === 3, 10.0);
$t->assertSame('the restart has run', $ran['manual_restarts'] ?? null, 3);

// This request opened without the cache while that restart waited, and runs
// on after it. With opcache.file_cache such a request loads the scripts it
// includes into the shared memory the restart has just reset, so it holds the
// next restart as a request opened with the cache does; this profile has no
// file cache and checks the holding alone. A server that does not count this
// request runs the next restart in the startup of one of the pool threads'
// replacements below, which then run with the cache.
$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled the next restart', $reset['reset'] ?? null, true);
$t->assertTrue('the pool threads were replaced and run without the cache', ox_poll_tasks(false, 5.0));
$other = ox_get_json('/tests/opcacherestart/fixture_status.php');
$t->assertSame('the next restart has not run under this request', $other['manual_restarts'] ?? null, 3);
$t->assertSame('the next restart is pending', $other['restart_pending'] ?? null, true);

$t->done();
