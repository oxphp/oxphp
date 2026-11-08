<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../opcacherestart/lib.php';

$t = new TestCase('worker_load_then_reset', 'opcacherestartworker');

// A worker is replaced for a restart at a moment with no request in flight, and
// a steady load of requests that park may leave it none. Starts that load,
// from a process of its own, and schedules a restart;
// worker_restart_ran_under_load checks the outcome. This request stays short: a
// worker with a request in flight cannot retire.
$before = ox_opcache_state();
$t->assertSame('premise: no restart is pending', $before['restart_pending'], false);

$base = ox_start_load('/tests/opcacherestart/fixture_sleep_load.php', 32, 0.3, 45.0, 'worker_load');
$t->assertTrue('the load is running', is_file("$base.started"));
usleep(500000);
file_put_contents("$base.before", json_encode(['restarts' => $before['manual_restarts'], 'reset_at' => time()]));

$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);

$t->done();
