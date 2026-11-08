<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../opcacherestart/lib.php';

$t = new TestCase('worker_restart_ran_under_load', 'opcacherestartworker');

// The log lines of this block wait for the workers to be replaced and the
// restart to run, so that no request sits on a worker meanwhile. One look here
// and no polling, for the same reason: a worker with a request in flight cannot
// be replaced. A restart that ran after the load had ended says nothing about a
// worker that never idles, and a worker replaced at the cost of refusing
// requests has not solved anything.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_worker_load';
$before = json_decode((string) file_get_contents("$base.before"), true);

$state = ox_get_json('/tests/opcacherestart/fixture_status.php', 5.0);
$t->assertSame('the restart has run', $state['manual_restarts'] ?? null, $before['restarts'] + 1);

// Each thread waits for the replacement of the one before it, not for the time
// a turn can last: a thread that went on waiting for that would leave the
// restart to run seconds later than it had to.
$ranAfter = ($state['last_restart_time'] ?? PHP_INT_MAX) - $before['reset_at'];
$t->assertGreaterThan('premise: the clock of the restart is the clock of the reset', $ranAfter, -2);
$t->assertLessThan('the threads were replaced one right after the other', $ranAfter, 5);
$t->meta('restart_ran_after_seconds', $ranAfter);

clearstatcache();
$t->assertFalse('the load was still running', is_file("$base.finished"));

$tally = ox_stop_load($base);
$t->assertNotNull('the load has ended', $tally);
$t->assertSame('no request of the load was refused', $tally['failed'] ?? null, []);
$t->assertGreaterThan('the load kept going', $tally['ok'] ?? 0, 50);
$t->meta('load', $tally);

$t->done();
