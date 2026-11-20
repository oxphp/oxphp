<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_off_plain_write_refused', 'hooksnetoff');

// With `streams` but not `net`, a write on a tcp:// stream reaches the socket hook
// from inside OpenSSL, which carries every such stream once it is loaded and has
// already chosen between its plain and TLS paths by then. A write that waited
// there for the fiber parked on the connection would carry on down the plain path
// even if the holder had switched the connection to TLS meanwhile, so it is
// refused at once instead. The same fwrite() waits under `net`, where the hook in
// front of OpenSSL's own write takes it before that choice.
$config = json_decode((string) @file_get_contents('http://127.0.0.1:9090/config'), true);
$t->assertSame(
    'the process published the categories it was given, without net',
    is_array($config) ? ($config['runtime_hooks'] ?? null) : null,
    ['sleep', 'streams']
);

$key = bin2hex(random_bytes(6));

$task = oxphp_async(function (string $key): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooks/fixture_net_plain_hold.php?key={$key} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
}, $key);

$deadline = microtime(true) + 3.0;
while (!isset($sharedState["plainhold_parked_{$key}"]) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
if (!isset($sharedState["plainhold_sock_{$key}"])) {
    $t->assertTrue('the holder opened the shared connection', false);
    $t->done();
}
// The parked edge is published just before the read starts.
oxphp_usleep(200_000);
$sock = $sharedState["plainhold_sock_{$key}"];

$t->assertSame(
    'OpenSSL carries the plain tcp:// stream the holder is parked on',
    stream_get_meta_data($sock)['stream_type'],
    'tcp_socket/ssl'
);

// Long enough that a write that waited would still be waiting when the holder's
// read ends.
stream_set_timeout($sock, 3);
$inside = !isset($sharedState["plainhold_done_{$key}"]);
$started = microtime(true);
$written = @fwrite($sock, "probe\n");
$waited = microtime(true) - $started;
$timedOut = stream_get_meta_data($sock)['timed_out'];
$stillInside = !isset($sharedState["plainhold_done_{$key}"]);
// The holder's read goes on after this: put back the timeout it set.
stream_set_timeout($sock, 4);

$t->assertTrue('the holder was parked on its read when the write started', $inside);
$t->assertTrue('and still was when it ended', $stillInside);
$t->assertFalse('the write was refused', $written);
$t->assertTrue('and reported as timed out', $timedOut);
$t->assertLessThan('at once, without waiting for the holder', $waited, 0.3);

unset($sharedState["plainhold_sock_{$key}"]);
$inner = oxphp_async_await($task);

$t->assertMatch('the holder was not ended by the refused write', $inner['body'], '#^HTTP/1\.[01] 200#');
$t->assertMatch(
    'and its read ran to its own timeout with nothing written in between',
    $inner['body'],
    '/plain-hold-done:\{"data":(""|false),"timed_out":true\}/'
);

$t->done();
