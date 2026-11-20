<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_closed_under_waiting_writer', 'hooksdb');

// A write waiting for the fiber parked on the same connection holds the php_stream
// and its php_netstream_data_t across the wait, as a parked read does. A third
// fiber closing that connection meanwhile frees both, and neither resuming the
// write nor failing it back is possible then: PHP's own write loop reads the
// stream again as soon as the write op returns. So the waiting request has to be
// ended, promptly and with a message that says why — the same answer a parked
// reader gets, which the holder here gets as well.
$key = bin2hex(random_bytes(6));
$log = "/tmp/oxphp-rawwrite-waiter-{$key}.log";

$fetch = static function (string $path): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})", 'at' => microtime(true)];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET {$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body, 'at' => microtime(true)];
};

$holder = oxphp_async($fetch, "/tests/hooksdb/fixture_raw_socket_write_hold.php?hold=6&key={$key}");
$deadline = microtime(true) + 3.0;
while (!isset($sharedState["rawwrite_parked_{$key}"]) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$waiter = oxphp_async($fetch, "/tests/hooksdb/fixture_raw_socket_write_waiter.php?key={$key}");
$deadline = microtime(true) + 3.0;
while (!isset($sharedState["rawwrite_waiting_{$key}"]) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
// The flag is set just before the write; this gives the write time to start
// waiting, well inside the two-second bound it would otherwise give up at.
oxphp_usleep(200_000);

$inside = isset($sharedState["rawwrite_waiting_{$key}"]) && !isset($sharedState["rawwrite_done_{$key}"]);
$t->assertTrue('the holder was parked on its reply and the writer had started its write', $inside);
if (!$inside) {
    $t->done();
}

$closedAt = microtime(true);
fclose($sharedState["rawwrite_sock_{$key}"]);
unset($sharedState["rawwrite_sock_{$key}"]);

$waited = oxphp_async_await($waiter);
$held = oxphp_async_await($holder);
$reported = is_file($log) ? (string) file_get_contents($log) : '';
@unlink($log);

$t->assertNotContains('the waiting writer did not go on with the stream freed under it', $waited['body'], 'raw-waiter-done:');
$t->assertMatch('it was ended with a server error instead', $waited['body'], '#^HTTP/1\.[01] 500#');
$t->assertContains(
    'its log names the close as what ended it',
    $reported,
    'was using was closed by another fiber while this one waited on it'
);
// Against the two-second bound the write would have given up at, counted from
// before the close: ending at the bound would be a write that never learned of it.
$t->assertLessThan('and it learned of the close then, not when its wait ran out', $waited['at'] - $closedAt, 1.0);

$t->assertMatch('the parked holder was ended by the close as well', $held['body'], '#^HTTP/1\.[01] 500#');
$t->assertNotContains('and did not read from the freed stream', $held['body'], 'raw-hold-done:');

// A fatal in one multiplexed request must not take the worker with it.
$next = $fetch('/tests/hooksdb/fixture_db_sleep.php');
$t->assertContains('the worker served the next request as usual', $next['body'], 'db-done');

$t->done();
