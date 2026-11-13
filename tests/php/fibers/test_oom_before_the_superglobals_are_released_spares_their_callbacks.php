<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A step of a request's end can run out of memory ahead of the steps that let go
// of what the request left — the session write ahead of the release of the
// closure of a task it never awaited, that release ahead of the release of its
// superglobals. That fails the request, and the steps after it still make the
// calls into user code they owe: the onClose() of a filter on a stream the
// closure held or one left in $_GET. While the engine's record of the error
// stands, it refuses every one of them, and the filter would go without being
// closed.
//
// ?case= task | session

$case = (string) ($_GET['case'] ?? '');
$t = new TestCase("oom_before_the_superglobals_are_released_spares_their_callbacks ($case)", 'fibers');

$run = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . '/oxphp-oom-release-' . $run;

$inner = static function (string $query) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$query: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_before_the_superglobals_are_released.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$query response", $resp);

    return $resp;
};

$resp = $inner("step=$case&run=$run");

$t->assertMatch("the out-of-memory error is the inner request's own failure", $resp, '#^HTTP/\S+ 500 #');
if ($case === 'task') {
    $t->assertTrue('the destructor that ran out of memory ran', is_file("$base.ran"));
} else {
    $t->assertTrue("the filter on the stream the task's closure held was closed", is_file("$base.task-closed"));
}
$t->assertTrue('the filter on the stream left in $_GET was closed', is_file("$base.closed"));

$probe = $inner("step=probe&run=$run");
$t->assertMatch('the next request answers 200', $probe, '#^HTTP/\S+ 200 #');
$t->assertContains('and runs its handler', $probe, 'PROBE-RAN');
$t->assertContains('and a callback', $probe, 'callback: [2,4]');

// On this worker, not on one started to replace it.
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

foreach (glob("$base.*") ?: [] as $file) {
    unlink($file);
}

$t->done();
