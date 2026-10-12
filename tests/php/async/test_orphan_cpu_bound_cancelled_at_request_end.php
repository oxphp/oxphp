<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('orphan_cpu_bound_cancelled_at_request_end', 'async');

// A request that ends without awaiting a task still running a loop that never
// suspends must cancel that task. The cancel flag alone reaches only a task the
// pool's scheduler loop gets to look at, which a spinning one never gives it the
// chance to do; the request's end has to break into it with an interrupt on
// its pool thread.
//
// The fixture is a separate request so that its end can be observed from here;
// it needs a second PHP worker (PHP_WORKERS >= 2) while this one waits for it.
// What is asserted is the exception the task was unwound by. A task nothing
// interrupts records "ran to its end" when its loop runs out after six seconds,
// or nothing yet if the polling window below closes first.
//
// Outside worker mode the end of the request waits for the task before the
// response is sent, so a task it fails to interrupt also holds the response for
// the 5 s budget that wait has. In worker mode that wait is deferred past the
// response, and the elapsed-time assertion holds either way.
$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_orphan_{$id}_0";

$t0 = microtime(true);
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 12);
fwrite($sock, "GET /tests/async/fixture_orphan_cpu_bound.php?id={$id}&orphans=1 HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
fclose($sock);
$elapsed = microtime(true) - $t0;

$t->assertContains('the fixture returned once its orphan was running', $resp, 'orphans-dispatched');

$deadline = microtime(true) + 2.0;
while (!file_exists("$base.result") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$result = file_exists("$base.result")
    ? (string) file_get_contents("$base.result")
    : '(no result within 2 s of the response)';

$t->assertSame(
    'the spinning orphan was unwound by the cancellation, not left to run out its loop',
    $result,
    'OxPHP\Async\AsyncException: Async task cancelled'
);
$t->assertLessThan('the response did not wait out the 5 s budget for the orphan', $elapsed, 2.0);
$t->meta('elapsed', round($elapsed, 3));

foreach (["$base.started", "$base.result"] as $file) {
    if (file_exists($file)) {
        unlink($file);
    }
}
$t->done();
