<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('task_declares_a_function_while_a_restart_waits', 'opcacherestart');

// First of three. What code from OPcache's shared memory leaves in the request
// that runs it outlives the call: a function it declares stays in that
// request's function table, pointing into the shared memory. A pool thread
// keeps one request for its whole life, so a pool thread that ran such a task
// holds restarts until it is replaced, even when it opened that request
// without the cache.
//
// This request has a restart scheduled and waits for the pool threads to be
// replaced by threads without the cache. Then it dispatches two tasks from its
// own code, which is in the shared memory: one that keeps its thread busy and
// holds the restart until thread_and_request_without_the_cache_hold_restarts
// lets it go, and one that declares a function on the other thread.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_adopt';
// The test helper turns every warning into an exception, @ included.
foreach (['started', 'go', 'report'] as $s) {
    if (is_file("$base.$s")) {
        unlink("$base.$s");
    }
}

$before = ox_opcache_state();
$t->assertSame('premise: this request runs with the cache', $before['opcache_enabled'], true);
$t->assertSame('premise: the previous restarts have run', $before['manual_restarts'], 2);
$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);
$t->assertTrue('premise: the pool threads run without the cache now', ox_poll_tasks(false, 5.0));

$holder = static function () use ($base): void {
    file_put_contents("$base.started", json_encode(['thread' => oxphp_worker_id()]));
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
oxphp_async($holder);

$deadline = microtime(true) + 3.0;
while (!file_exists("$base.started") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$started = is_file("$base.started") ? json_decode((string) file_get_contents("$base.started"), true) : null;
$t->assertTrue('premise: the holding task has started', is_array($started));

// The holding task sleeps without yielding its thread, so this one runs on the
// other.
$declarer = static function (): array {
    function ox_opcacherestart_declared(): bool
    {
        return true;
    }
    $s = opcache_get_status(false);
    return [
        'thread' => oxphp_worker_id(),
        'opcache_enabled' => is_array($s) && $s['opcache_enabled'],
        'declared' => function_exists('ox_opcacherestart_declared'),
    ];
};
$declared = oxphp_async_await(oxphp_async($declarer), 5.0);
$t->assertSame('the task declared the function', $declared['declared'] ?? null, true);
$t->assertSame('premise: on a thread without the cache', $declared['opcache_enabled'] ?? null, false);
$t->assertTrue(
    'premise: on the thread the holding task leaves free',
    isset($declared['thread']) && $declared['thread'] !== ($started['thread'] ?? null),
);

$t->done();
