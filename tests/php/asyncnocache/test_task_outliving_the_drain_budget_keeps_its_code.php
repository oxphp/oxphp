<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_outliving_the_drain_budget_keeps_its_code', 'asyncnocache');

// Without OPcache the code of a task's closure lives in the memory of the
// request that dispatched it, and that memory is released when the request
// ends. A task the end of the request cancelled but which keeps running past
// the drain budget must therefore be waited for until it stops: released after
// the budget, as a task whose code is in OPcache's shared memory is, it would go
// on running code that no longer exists.
//
// The fixture's task is a static closure — no `$this` — whose single await
// times out after 0.2 s; the task catches the cancellation and keeps working
// for another eight seconds, three past the budget. So:
//
//   * the end of the request waited for the task, past the budget: it finished
//     before the client received the response (`ended_at < resp_at`), and the
//     response came after it (`elapsed > 7`) — a request end that stops at the
//     budget answers in about 5.2 s;
//   * premise, read after the fact: the task really did outlive the budget
//     (`ended_at - t0 > 7`);
//   * the server is still serving afterwards.
//
// With OPcache the same fixture's response is held to the budget and no longer
// — `async/test_await_timeout_waits_for_the_task_at_request_end`.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_awaittmo_{$id}";

$t0 = microtime(true);
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 20);
fwrite($sock, "GET /tests/async/fixture_await_timeout_orphan.php?id={$id}&linger=8 HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
fclose($sock);
$resp_at = microtime(true);
$elapsed = $resp_at - $t0;

// The task ends about 8.2 s after `t0`, on its own schedule, whenever the
// response came.
$deadline = $t0 + 12.0;
while (!file_exists("$base.ended_at") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$ended_at = is_file("$base.ended_at") ? (float) file_get_contents("$base.ended_at") : 0.0;
foreach (['started', 'ended_at'] as $s) {
    if (is_file("$base.$s")) {
        @unlink("$base.$s");
    }
}

$t->assertContains('the single await timed out as expected', $resp, 'await-timed-out');
$t->assertGreaterThan('the task recorded a finish time', $ended_at, 0.0);
$t->assertGreaterThan('the task outlived the drain budget', $ended_at - $t0, 7.0);
$t->assertLessThan('the request end waited for the task', $ended_at, $resp_at);
$t->assertGreaterThan('the response came after the budget', $elapsed, 7.0);
$t->meta('elapsed', round($elapsed, 3));
$t->meta('task_ran_for', round($ended_at - $t0, 3));

$after = @file_get_contents(
    'http://127.0.0.1/tests/asyncnocache/fixture_lookup_array_object.php',
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);
$t->assertSame('the server still runs tasks afterwards', $after, 'ArrayObject');

$t->done();
