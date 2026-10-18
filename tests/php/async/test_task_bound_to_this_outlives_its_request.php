<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_bound_to_this_outlives_its_request', 'async');

// A non-static closure dispatched from a method runs bound to `$this`. The task
// must own that `$this` itself — it must not be an object that lives in the
// dispatching request's memory, because the task can outlive that request and
// its memory is torn down at the request's end. The bound `$this` is a
// BorrowedProxy, and it has to be one the async pool created and owns, not the
// request's.
//
// The fixture dispatches such a task and returns without awaiting it. Outside
// worker mode the request end waits up to the drain budget for the task, so:
//
//   * the response comes back around the budget (`4 <= elapsed < 7`);
//   * premise, both edges of the window: the task was cancelled before the
//     response was produced (`caught_at < resp_at`) and was still touching
//     `$this` after the response went out (`ended_at > resp_at`) — i.e. after
//     the dispatching request's heap was gone;
//   * liveness: across that whole window the task's `$this` stayed a
//     BorrowedProxy, its class intact. This alone cannot tell an object the
//     task owns from freed memory nothing has reused yet; what makes `$this`
//     the task's own is that the async pool creates it on the task's thread.
//
// The last check asserts that dispatching a bound closure raises no
// deprecation: the proxy gets no dynamic `promiseId` property.
//
// The fixture is a separate request so its end can be observed from here; it
// needs a second PHP worker (PHP_WORKERS >= 2) while this one waits.
$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_thisbound_{$id}";

$t0 = microtime(true);
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 15);
fwrite($sock, "GET /tests/async/fixture_this_bound_orphan.php?id={$id} HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
fclose($sock);
$resp_at = microtime(true);
$elapsed = $resp_at - $t0;

$t->assertContains('the fixture dispatched the bound orphan', $resp, 'orphan-dispatched');

$t->assertGreaterThan('the response waited for the drain budget', $elapsed, 4.0);
$t->assertLessThan('the response did not exceed the budget', $elapsed, 7.0);

$deadline = microtime(true) + 6.0;
while (!file_exists("$base.ended_at") && microtime(true) < $deadline) {
    usleep(50000);
    clearstatcache();
}

$caught_at = is_file("$base.caught_at") ? (float) file_get_contents("$base.caught_at") : 0.0;
$ended_at = is_file("$base.ended_at") ? (float) file_get_contents("$base.ended_at") : 0.0;
$result = is_file("$base.result") ? trim((string) file_get_contents("$base.result")) : '(no result)';
$dispatchErr = is_file("$base.dispatch_err") ? (string) file_get_contents("$base.dispatch_err") : '';

$t->assertGreaterThan('the task recorded a cancellation', $caught_at, 0.0);
$t->assertGreaterThan('the task was cancelled before the response', $resp_at, $caught_at);
$t->assertGreaterThan('the task outlived the response', $ended_at, $resp_at);

$t->assertSame('the task\'s $this stayed a live pool-owned BorrowedProxy', $result, 'ok');

$t->assertNotContains('dispatching a bound closure raised no deprecation', $dispatchErr, 'promiseId');

$t->meta('elapsed', round($elapsed, 3));
$t->meta('result', $result);
$t->meta('dispatch_err', $dispatchErr === '' ? '(none)' : $dispatchErr);

foreach (['started', 'caught_at', 'ended_at', 'result', 'dispatch_err'] as $s) {
    $f = "$base.$s";
    if (is_file($f)) {
        @unlink($f);
    }
}
$t->done();
