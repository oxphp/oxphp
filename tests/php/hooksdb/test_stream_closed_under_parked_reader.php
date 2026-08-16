<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_closed_under_parked_reader', 'hooksdb');

// A socket read that parks holds the php_stream and its php_netstream_data_t in a
// C frame across the suspension, and the worker goes on serving other fibers
// meanwhile. If one of them closes that same stream — one connection shared by
// every request is the shape a worker-mode application has — PHP frees both
// structs, and the reader resuming into them writes to memory that is gone.
// Returning an error instead is not enough either: php_stream_read() reads the
// stream again after the read op hands control back.
//
// So what is asserted here is that the parked request is ended, promptly and
// diagnosably, rather than resumed onto freed memory.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    // Past the holder's own ten-second read deadline, so a build that never tells
    // it about the close is read to the end rather than cut off here — the failure
    // then says "it waited out its deadline" instead of "this socket gave up
    // first". Still inside the runner's per-test ceiling, which would say neither.
    stream_set_timeout($sock, 13);
    fwrite($sock, "GET /tests/hooksdb/fixture_raw_socket_park.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

// The holder publishes a flag between its write and its read, so this waits until
// it has parked instead of sleeping long enough that it ought to have: a fixed
// sleep a loaded host outruns leaves this request closing a stream nobody is
// parked on, which proves nothing. The ceiling is well under the holder's own
// ten-second deadline, so even a build where the flag never arrives reports
// inside the runner's per-test limit.
$deadline = microtime(true) + 3.0;
while (!($sharedState['rawsock_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$opened = isset($sharedState['rawsock']);
$parked = $opened && ($sharedState['rawsock_parked'] ?? false);
$t->assertTrue('the holding request opened the shared stream before this one woke', $opened);
$t->assertTrue('and it reached the read that parks it', $parked);
if (!$parked) {
    // Everything below this point closes and inspects the stream the holder was
    // to have left here. Without it they raise rather than fail, and the runner
    // is handed an error carrying no assertion at all — losing the one line that
    // would have said which precondition was missing.
    $t->done();
}

$started = microtime(true);
fclose($sharedState['rawsock']);
unset($sharedState['rawsock'], $sharedState['rawsock_parked']);

$inner = oxphp_async_await($task);
$waited = microtime(true) - $started;

// The holder must not have come back from its read at all: the value it would
// print is read out of a stream that no longer exists.
$t->assertNotContains(
    'the parked request did not go on using the stream freed under it',
    $inner['body'],
    'raw-park-done:'
);
$t->assertMatch(
    'it was ended with a server error instead',
    $inner['body'],
    '#^HTTP/1\.[01] 500#'
);

// The other half of the guarantee, and the half a status assertion alone does not
// cover: the holder learned of the close when it happened rather than sitting out
// its own read deadline. Closing a descriptor drops it from the readiness
// instance without a word, so nothing wakes the parked fiber unless the close
// itself does — and for a client whose stream timeout is hours (mysqlnd's default
// is a day) "at the deadline" is indistinguishable from never. The holder's own
// deadline is ten seconds against a green of about one, so the ceiling has the
// whole gap to sit in rather than a margin a loaded host could close.
$t->assertLessThan(
    'and it learned of the close then, not at its own read deadline ten seconds later',
    $waited,
    5.0
);

// The worker kept serving: a fatal in one multiplexed request must not take the
// others, or the next ones, with it.
$after = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_db_sleep.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});
$next = oxphp_async_await($after);

$t->assertContains('the worker served the next request as usual', $next['body'], 'db-done');

$t->done();
