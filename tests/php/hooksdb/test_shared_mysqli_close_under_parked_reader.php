<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_mysqli_close_under_parked_reader', 'hooksdb');

// The same defect as test_stream_closed_under_parked_reader, reached through a
// client rather than through a raw resource — `$wpdb->check_connection()` calling
// mysqli_close() on the connection every request of the worker shares is what put
// it on the list in the first place.
//
// close() is a guarded entry point, so it waits for the fiber holding the
// connection; but the wait is bounded by this call's own deadline, and a holder
// parked on a long query outlives it. Past the bound the call is handed to the
// original handler — which closes the connection out from under the parked
// reader. The bound in this image is two seconds (default_socket_timeout), the
// holder's query nine, so the give-up branch is the one taken here.
//
// That reading only holds while the two limits differ. This image starts with
// max_execution_time at 0, which imposes no limit at all, so without the line
// below default_socket_timeout would be both the smaller and the larger and the
// timing check further down would pass even on a regression that started taking
// the larger. A request limit of thirty seconds separates them again without
// touching the startup value the rest of the profile is built on:
// max_execution_time is read as the request currently has it, so the wait below
// must still come out at two seconds and not at thirty.
set_time_limit(30);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_mysqli_park.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

// Waited for rather than slept past, and stopped cleanly if it never happens:
// closing a connection nobody is parked on proves nothing, and every line below
// dereferences a connection the holder was to have left here — without the stop
// they raise, and the report becomes an error with no failed check in it.
$deadline = microtime(true) + 3.0;
while (!($sharedState['mysqli_doomed_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$built = isset($sharedState['mysqli_doomed']);
$parked = $built && ($sharedState['mysqli_doomed_parked'] ?? false);
$t->assertTrue('the holding request built the shared connection before this one woke', $built);
$t->assertTrue('and it reached the query that parks it', $parked);
if (!$parked) {
    $t->done();
}

$started = microtime(true);
$sharedState['mysqli_doomed']->close();
unset($sharedState['mysqli_doomed'], $sharedState['mysqli_doomed_parked']);
$closeTook = microtime(true) - $started;

$inner = oxphp_async_await($task);
$waited = microtime(true) - $started;

// It really was the give-up branch: close() waited its bound before delegating,
// so the holder was still parked on the connection when it was closed. Both ends
// of that wait, because one without the other is satisfied by the two things this
// is here to tell apart — a close that never waited at all, and one that waited
// out max_execution_time instead of the smaller limit.
$t->assertGreaterThan('close() waited for the holder before giving up', $closeTook, 1.5);
$t->assertLessThan(
    'and gave up at the smaller of the two limits rather than at the request one',
    $closeTook,
    5.0
);

$t->assertNotContains(
    'the parked request did not go on using the connection closed under it',
    $inner['body'],
    'mysqli-park-done:'
);
$t->assertMatch(
    'it was ended with a server error instead',
    $inner['body'],
    '#^HTTP/1\.[01] 500#'
);
// Its own read deadline is mysqlnd.net_read_timeout — a day — so without being
// told of the close it would not come back at all. Nine seconds is the answer to
// its query arriving; anything under that is the close having reached it.
$t->assertLessThan(
    'and it learned of the close rather than waiting on a descriptor nobody will answer',
    $waited,
    5.0
);

$t->done();
