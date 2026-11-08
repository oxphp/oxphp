<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('task_holds_the_restart_past_its_request', 'opcacherestart');

// Second of three. The request that dispatched the task has ended and the pool
// threads hold nothing, so the task alone stands between the pending restart
// and the startup of this request. A server that counts requests but not tasks
// runs the restart here, under a task still running code from the shared
// memory: this request then reports it done.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_task';
$started = is_file("$base.started") ? json_decode((string) file_get_contents("$base.started"), true) : null;
$t->assertTrue('premise: the task is running', is_array($started));
$t->assertSame('premise: the task runs on a thread without the cache', $started['opcache_enabled'] ?? null, false);

$mine = ox_opcache_state();
$t->assertSame('the restart is still pending', $mine['restart_pending'], true);
$t->assertSame('the restart has not run', $mine['manual_restarts'], 1);
$t->assertSame('this request runs without the cache', $mine['opcache_enabled'], false);

// The restart was scheduled before this request started. Three more seconds
// put its wait past opcache.force_restart_timeout (2 s in this profile) and one
// turn of the server's once-a-second check, which then logs that the restart is
// still waiting (opcacherestart suite, the line after this block).
sleep(3);

touch("$base.go");
$deadline = microtime(true) + 5.0;
while (!file_exists("$base.report") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$report = is_file("$base.report") ? json_decode((string) file_get_contents("$base.report"), true) : null;
$t->assertSame('the restart did not run under the task', $report['manual_restarts'] ?? null, 1);

$t->done();
