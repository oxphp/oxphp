<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('load_then_reset_under_task_load', 'opcacherestart');

// A pool thread is replaced for a restart at a moment it runs no task, and a
// steady load of tasks that park may leave it none. Starts that load, from a
// process of its own, and then schedules a restart; restart_ran_under_task_load
// checks the outcome while the load is still running. This request is short:
// run with the cache, it holds the restart until it ends.
$before = ox_opcache_state();
$t->assertSame('premise: no restart is pending', $before['restart_pending'], false);
$t->assertTrue('premise: both pool threads run with the cache', ox_poll_tasks(true, 10.0));

$base = ox_start_load('/tests/opcacherestart/fixture_task_load.php', 3, 0.45, 40.0, 'task_load');
$t->assertTrue('the load is running', is_file("$base.started"));
usleep(500000);
file_put_contents("$base.before", json_encode(['restarts' => $before['manual_restarts'], 'reset_at' => time()]));

$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);

$t->done();
