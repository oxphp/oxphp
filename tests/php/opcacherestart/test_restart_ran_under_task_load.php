<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('restart_ran_under_task_load', 'opcacherestart');

// The log lines of the block above have seen the pool threads replaced and the
// restart run. It all has to have happened with the load still on: a restart
// that ran once the load had ended says nothing about a thread that never idles.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_task_load';
$before = json_decode((string) file_get_contents("$base.before"), true);

$after = ox_poll_state(
    static fn (array $s): bool => $s['manual_restarts'] === $before['restarts'] + 1 && $s['opcache_enabled'],
    5.0,
);
$t->assertSame('the restart has run', $after['manual_restarts'] ?? null, $before['restarts'] + 1);
$t->assertSame('a request started after it runs with the cache', $after['opcache_enabled'] ?? null, true);
$t->assertTrue('both pool threads run tasks with the cache again', ox_poll_tasks(true, 15.0));

// Each thread waits for the replacement of the one before it, not for the time
// a turn can last: a thread that went on waiting for that would leave the
// restart to run seconds later than it had to.
$ranAfter = ($after['last_restart_time'] ?? PHP_INT_MAX) - $before['reset_at'];
$t->assertGreaterThan('premise: the clock of the restart is the clock of the reset', $ranAfter, -2);
$t->assertLessThan('the threads were replaced one right after the other', $ranAfter, 5);
$t->meta('restart_ran_after_seconds', $ranAfter);

clearstatcache();
$t->assertFalse('the load was still running', is_file("$base.finished"));

$tally = ox_stop_load($base);
$t->assertNotNull('the load has ended', $tally);
$t->meta('load', $tally);
$t->assertSame('no request of the load was refused', $tally['failed'] ?? null, []);

$t->done();
