<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('restart_held_for_the_pool_calls', 'opcacherestart');

// First of two. A request opened without the cache becomes a holder of the
// restart it opened during once it runs code that holds restarts; a call into
// a Shared\Pool callback another thread made is such code, and a call the pool
// refuses runs none of it.
//
// This request schedules a restart, waits for the pool threads to be replaced
// by threads without the cache, and leaves a task from its own code, which is
// in the shared memory, holding the restart until
// refused_pool_call_does_not_hold_the_restart lets it go.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_poolcall';
// The test helper turns every warning into an exception, @ included.
foreach (['started', 'go', 'report', 'held', 'go2'] as $s) {
    if (is_file("$base.$s")) {
        unlink("$base.$s");
    }
}

$before = ox_opcache_state();
$t->assertSame('premise: this request runs with the cache', $before['opcache_enabled'], true);
$t->assertSame('premise: no restart is pending', $before['restart_pending'], false);
$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);
$t->assertTrue('premise: the pool threads run without the cache now', ox_poll_tasks(false, 5.0));

$holder = static function () use ($base): void {
    file_put_contents("$base.started", json_encode(['thread' => oxphp_worker_id()]));
    $until = microtime(true) + 30.0;
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
    file_put_contents("$base.report", json_encode(['ended' => true]));
};
oxphp_async($holder);

$deadline = microtime(true) + 3.0;
while (!file_exists("$base.started") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$t->assertTrue('premise: the holding task has started', is_file("$base.started"));
$t->meta('restarts_before', $before['manual_restarts']);

$t->done();
