<?php

declare(strict_types=1);

// A worker that retires, here by Worker::scheduleExit(), kills the requests still
// parked on it, and one parked inside a cURL transfer is killed only once libcurl
// has ended the transfer and returned. Killed from inside libcurl's wait, it would
// leave the handle inside curl_exec() as far as libcurl is concerned, for good:
// every later call on it refused, and the handle never freed, with its connection.
//
// This request sends the fixture a request of its own, which parks in curl_exec()
// against a listener that never answers, and then schedules the exit. The fixture's
// shutdown function records what the kill left; the probe on the next line reads
// it on the worker that replaced this one.
//
// With ?throw=1 the listener sends the start of an answer, the fixture's write
// callback throws on it, and the kill comes while the transfer waits for the rest
// with that exception pending. The exception has to come back out of the wait with
// the transfer: the request is ended only once libcurl returns, and until then the
// transfer, or the callback a nested cURL call runs in, goes on running.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$t = new TestCase('curl_transfer_ended_by_a_retire_leaves_its_handle_usable', 'fibers');

OxphpBailoutLeak::arm('curl_retire');
unset($sharedState['curl_retire_parked'], $sharedState['curl_retire_threw']);
$throw = isset($_GET['throw']);

// The kernel accepts the connection into the backlog, and nobody ever answers.
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
$t->assertTrue("a listener to park the transfer on: {$errstr}", $server !== false);
if ($server === false) {
    $t->done();
}
$port = (int) substr((string) strrchr((string) stream_socket_get_name($server, false), ':'), 1);
$sharedState['curl_retire_server'] = $server;

$client = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue("connected to this server: {$errstr}", $client !== false);
if ($client === false) {
    $t->done();
}
$mode = $throw ? '&throw=1' : '';
fwrite($client, "GET /tests/fibers/fixture_curl_parked_for_a_retire.php?port={$port}{$mode} HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
// Never read: the fixture's answer comes as the worker goes.
$sharedState['curl_retire_client'] = $client;

$deadline = microtime(true) + 3.0;
while (!($sharedState['curl_retire_parked'] ?? false) && microtime(true) < $deadline) {
    usleep(20_000);
}
if ($throw) {
    $conn = stream_socket_accept($server, 3.0);
    $t->assertTrue('the fixture connected to the listener', $conn !== false);
    if ($conn !== false) {
        fwrite($conn, "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nthe start of the body");
        $sharedState['curl_retire_conn'] = $conn;
    }
    $deadline = microtime(true) + 3.0;
    while (!($sharedState['curl_retire_threw'] ?? false) && microtime(true) < $deadline) {
        usleep(20_000);
    }
    $t->assertTrue('the write callback threw', (bool) ($sharedState['curl_retire_threw'] ?? false));
}
// Inside the wait for the answer, not about to start it.
usleep(200_000);
$t->assertTrue('the fixture started its transfer', (bool) ($sharedState['curl_retire_parked'] ?? false));

OxPHP\Server\Worker::current()->scheduleExit();

$t->done();
