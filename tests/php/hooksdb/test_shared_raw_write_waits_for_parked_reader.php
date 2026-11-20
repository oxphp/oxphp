<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_raw_write_waits_for_parked_reader', 'hooksdb');

// A command written by one fiber on a connection another fiber is parked on for
// its reply. A userland client such as Predis has no guarded entry point in front
// of its writes, so the socket level is all that keeps the two exchanges apart —
// and what the client does with a write that fails is the whole question: Predis
// reads a write returning false or 0 as a dead connection and closes it, which
// frees the stream under the fiber parked on it and ends that request too. So the
// write has to wait for the holder's exchange to end, not fail.
//
// This request does what Predis does: a write, and on failure fclose().
$key = bin2hex(random_bytes(6));

$task = oxphp_async(function (string $key): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_raw_socket_write_hold.php?hold=1&key={$key} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
}, $key);

$deadline = microtime(true) + 3.0;
while (!isset($sharedState["rawwrite_parked_{$key}"]) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

// Read at the moment of the write, not before the wait above: the holder has to
// be inside its exchange when the write starts, or a write that goes straight
// through says nothing.
$inside = isset($sharedState["rawwrite_parked_{$key}"]) && !isset($sharedState["rawwrite_done_{$key}"]);
$t->assertTrue('the holder was parked on its reply when the write started', $inside);
if (!$inside) {
    $t->done();
}

$sock = $sharedState["rawwrite_sock_{$key}"];
$command = "ECHO b-probe\r\n";
$written = @fwrite($sock, $command);
$holderDoneAtReturn = isset($sharedState["rawwrite_done_{$key}"]);
$timedOut = stream_get_meta_data($sock)['timed_out'];

$reply = '';
if ($written === false || $written === 0) {
    fclose($sock);
} else {
    $reply = (string) fgets($sock) . (string) fgets($sock);
}
unset($sharedState["rawwrite_sock_{$key}"]);

$inner = oxphp_async_await($task);

$t->assertSame('the write was not failed back to the client', $written, strlen($command));
$t->assertFalse('and was not reported as timed out', $timedOut);

// The mechanism: by the time the write returned, the holder had read its reply.
// A write that went straight onto the wire would return with the holder still
// parked, its command landing in the middle of the holder's exchange.
$t->assertTrue('it returned only after the holder had read its reply', $holderDoneAtReturn);
$t->assertSame('and this request read the answer to its own command', $reply, "\$7\r\nb-probe\r\n");

$t->assertMatch('the holder was not ended by this request', $inner['body'], '#^HTTP/1\.[01] 200#');
$t->assertContains('and got the answer to its own command', $inner['body'], 'raw-hold-done:"*-1');

$t->done();
