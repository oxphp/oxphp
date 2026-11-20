<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_raw_write_wait_gives_up', 'hooksdb');

// The other half of a write waiting for a fiber parked on the same connection:
// the wait is bounded, and by the smaller of two limits — the stream's own
// timeout, which is what the caller asked a write to take at most, and the bound
// every connection claim has (the smaller of max_execution_time and the
// default_socket_timeout the process started with; this image ships two
// seconds). Each limit is made the smaller one in turn, against a holder parked
// for longer than both, so each phase fails if the wait ignores its limit, and
// both fail if the write is refused without waiting.
//
// A write that gives up is failed the way a timed-out one is, and nothing is
// written: the holder's exchange comes out of it untouched.
//
// And only a userland fwrite() or fputs() on a stream with no write filter waits
// at all. fprintf() stands in for a C client such as mysqlnd, whose write is told
// apart from fwrite() by the call alone, and both it and an fwrite() through a
// filter are refused at once.
$key = bin2hex(random_bytes(6));

$task = oxphp_async(function (string $key): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_raw_socket_write_hold.php?hold=6&key={$key} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
}, $key);

$deadline = microtime(true) + 3.0;
while (!isset($sharedState["rawwrite_parked_{$key}"]) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
if (!isset($sharedState["rawwrite_sock_{$key}"])) {
    $t->assertTrue('the holder opened the shared connection', false);
    $t->done();
}
$sock = $sharedState["rawwrite_sock_{$key}"];

// Max_execution_time pinned well above both, so the claim bound is the two
// seconds of default_socket_timeout whatever the image starts it at.
set_time_limit(30);

$attempt = static function ($sock, int $sec, int $usec, ?\Closure $write = null) use (&$sharedState, $key): array {
    // The stream timeout is the stream's, not this fiber's, and the holder's read
    // goes on after this test: put back what the holder set.
    stream_set_timeout($sock, $sec, $usec);
    $inside = isset($sharedState["rawwrite_parked_{$key}"]) && !isset($sharedState["rawwrite_done_{$key}"]);
    $started = microtime(true);
    $written = $write !== null ? $write($sock) : @fwrite($sock, "ECHO b-probe\r\n");
    $waited = microtime(true) - $started;
    $timedOut = stream_get_meta_data($sock)['timed_out'];
    $stillInside = !isset($sharedState["rawwrite_done_{$key}"]);
    stream_set_timeout($sock, 10);

    return compact('inside', 'written', 'waited', 'timedOut', 'stillInside');
};

// The stream's timeout is the smaller limit.
$short = $attempt($sock, 0, 500_000);
$t->assertTrue('the holder was parked on its reply when the first write started', $short['inside']);
$t->assertTrue('and still was when it ended', $short['stillInside']);
$t->assertFalse('the write gave up', $short['written']);
$t->assertTrue('and was reported as timed out', $short['timedOut']);
$t->assertGreaterThan('it waited, rather than being refused at once', $short['waited'], 0.35);
$t->assertLessThan('and gave up at its own half-second timeout, not at the claim bound', $short['waited'], 1.5);

// The claim bound is the smaller limit.
$long = $attempt($sock, 10, 0);
$t->assertTrue('the holder was parked on its reply when the second write started', $long['inside']);
$t->assertTrue('and still was when it ended', $long['stillInside']);
$t->assertFalse('the second write gave up', $long['written']);
$t->assertTrue('and was reported as timed out', $long['timedOut']);
$t->assertGreaterThan('it waited out the claim bound', $long['waited'], 1.5);
$t->assertLessThan('and gave up there, not at its own ten-second timeout or the holder\'s end', $long['waited'], 2.9);

// Each limit is ten seconds here, so a write that waited would take the two-second
// claim bound.
$printed = $attempt($sock, 10, 0, static fn ($sock) => @fprintf($sock, "ECHO c-probe\r\n"));
$t->assertTrue('the holder was parked on its reply when fprintf() started', $printed['inside']);
$t->assertTrue('and still was when it ended', $printed['stillInside']);
$t->assertTrue('fprintf() was reported as timed out', $printed['timedOut']);
$t->assertLessThan('fprintf() was refused at once, without waiting for the holder', $printed['waited'], 0.3);

$filtered = $attempt($sock, 10, 0, static function ($sock) {
    $filter = stream_filter_append($sock, 'string.toupper', STREAM_FILTER_WRITE);
    $written = @fwrite($sock, "echo d-probe\r\n");
    stream_filter_remove($filter);

    return $written;
});
$t->assertTrue('the holder was parked on its reply when the filtered fwrite() started', $filtered['inside']);
$t->assertTrue('and still was when it ended', $filtered['stillInside']);
$t->assertTrue('the filtered fwrite() was reported as timed out', $filtered['timedOut']);
$t->assertLessThan('the filtered fwrite() was refused at once, without waiting for the holder', $filtered['waited'], 0.3);

unset($sharedState["rawwrite_sock_{$key}"]);
$inner = oxphp_async_await($task);

$t->assertMatch('the holder was not ended by the writes that gave up', $inner['body'], '#^HTTP/1\.[01] 200#');
$t->assertContains('and got the answer to its own command', $inner['body'], 'raw-hold-done:"*-1');

$t->done();
