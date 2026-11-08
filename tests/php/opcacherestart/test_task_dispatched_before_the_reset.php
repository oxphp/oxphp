<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('task_dispatched_before_the_reset', 'opcacherestart');

// First of three requests. The code of an oxphp_async() task whose closure is
// in OPcache's shared memory stays there for the task, not in the memory of the
// request that dispatched it, and the task can outlive that request. Such a
// task holds a restart like a request does.
//
// This request has a restart scheduled and holds it while the pool threads,
// idle, are replaced; their replacements open without the cache. Then it dispatches a
// task from its own code — in the shared memory — and ends without waiting for
// it: the task catches the cancellation the end of the request sends and keeps
// running until task_holds_the_restart_past_its_request lets it go. From the end
// of this request on, nothing but the task itself holds the restart.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_task';
// The test helper turns every warning into an exception, @ included.
foreach (['started', 'go', 'report'] as $s) {
    if (is_file("$base.$s")) {
        unlink("$base.$s");
    }
}

$before = ox_opcache_state();
$t->assertSame('premise: this request runs with the cache', $before['opcache_enabled'], true);
$t->assertSame('premise: the previous restart has run', $before['manual_restarts'], 1);
// From another request: the server acts on a restart once the request that
// scheduled it has closed, and this one has to stay open.
$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);

$t->assertTrue('premise: the pool threads run without the cache now', ox_poll_tasks(false, 5.0));

$task = static function () use ($base): void {
    $s = opcache_get_status(false);
    file_put_contents("$base.started", json_encode([
        'opcache_enabled' => is_array($s) && $s['opcache_enabled'],
        'manual_restarts' => is_array($s) ? $s['opcache_statistics']['manual_restarts'] : -1,
    ]));
    $until = microtime(true) + 25.0;
    while (microtime(true) < $until) {
        try {
            while (microtime(true) < $until) {
                clearstatcache();
                if (file_exists("$base.go")) {
                    break 2;
                }
                usleep(20000);
            }
        } catch (\Throwable) {
            // The end of the request that dispatched this task cancels it.
        }
    }
    $s = opcache_get_status(false);
    file_put_contents("$base.report", json_encode([
        'manual_restarts' => is_array($s) ? $s['opcache_statistics']['manual_restarts'] : -1,
    ]));
};
oxphp_async($task);

$deadline = microtime(true) + 3.0;
while (!file_exists("$base.started") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$t->assertTrue('the task has started', file_exists("$base.started"));

$t->done();
