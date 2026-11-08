<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../opcacherestart/lib.php';

$t = new TestCase('worker_every_worker_runs_cached', 'opcacherestartworker');

// After the restart, no worker is left without the cache: this one and the
// other one both run with it. The worker that replaced the last holder runs the
// restart; the one that booted while the restart waited has to be replaced once
// more. A server that replaces only the workers holding the restart leaves that
// one without the cache for good, and one of the two checks below fails —
// worker_restart_converges alone can pass by asking only the worker that ran
// the restart.
$mine = ox_opcache_state();
$t->assertSame('this worker runs with the cache', $mine['opcache_enabled'], true);
$t->assertSame('after the one restart', $mine['manual_restarts'], 1);

// This worker is busy with this request: the other one answers.
$streak = 0;
$last = null;
for ($i = 0; $i < 5; $i++) {
    $last = ox_get_json('/tests/opcacherestart/fixture_status.php', 5.0);
    if ($last !== null && $last['opcache_enabled'] && $last['manual_restarts'] === 1) {
        $streak++;
    }
}
$t->assertSame('the other worker runs with the cache', $streak, 5);
$t->meta('last', $last);

$t->done();
