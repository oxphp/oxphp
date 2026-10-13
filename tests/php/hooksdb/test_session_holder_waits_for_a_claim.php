<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('session_holder_waits_for_a_claim', 'hooksdb');

// A request with its session open waits for a connection the worker shares as
// any other request does: until the request holding the connection is done
// with it, not until the wait gives up.
//
// One session at a time is open on a worker, and while a request is parked with
// its session open the worker admits no new request. Everything already on the
// worker goes on running — the request holding the connection included. Were
// the worker to stop running them instead, the session's own request would wait
// for a connection whose holder could not run to give it back, and both would
// sit there until the wait's bound ran out.
//
// A holder takes the connection with a query that runs for a second. Inside
// that second a second request opens its session and queries the same
// connection. It has to get its answer once the holder is done — well inside
// the bound, which this profile sets to two seconds.

// Its own key, created here before either request runs: see
// test_shared_db_concurrent_queries.php for why the fixture does not create it.
$sharedState['pdo_session_claim'] ??= new PDO(
    'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
    getenv('DB_USER') ?: 'appuser',
    getenv('DB_PASS') ?: 'apppass',
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);

$send = static function (string $query, string $cookie) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        throw new \RuntimeException("inner connect failed: $errstr ($errno)");
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_session_claim.php?key=pdo_session_claim&$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\n"
        . ($cookie === '' ? '' : "Cookie: PHPSESSID=$cookie\r\n")
        . "Connection: close\r\n\r\n");

    return $sock;
};
$read = static function ($sock): mixed {
    $raw = (string) stream_get_contents($sock);
    fclose($sock);
    $split = strpos($raw, "\r\n\r\n");

    return json_decode($split === false ? $raw : substr($raw, $split + 4), true);
};

unset($sharedState['session_claim_holding']);
$holdSock = $send('role=hold', '');
// Waiting parks this request, which lets the worker take the holder.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['session_claim_holding']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$sessionId = str_repeat('b1', 16);
$sessionSock = $send('role=session', $sessionId);

$session = $read($sessionSock);
$hold = $read($holdSock);

$t->meta('hold', $hold);
$t->meta('session', $session);

$t->assertTrue('the holder answered with JSON', is_array($hold));
$t->assertTrue('the session request answered with JSON', is_array($session));
if (!is_array($hold) || !is_array($session)) {
    $t->done();
}

$t->assertSame('the holder\'s query ran to its end', (string) ($hold['result'] ?? ''), '0');

// The premise: the session request queried while the holder had the connection,
// and had its session open as it did.
$t->assertGreaterThan(
    'the session request queried after the holder had taken the connection',
    (float) ($session['query_started_at'] ?? 0),
    (float) ($hold['query_started_at'] ?? INF)
);
$t->assertLessThan(
    'and before the holder was done with it',
    (float) ($session['query_started_at'] ?? INF),
    (float) ($hold['query_done_at'] ?? 0)
);
$t->assertTrue('with its session open', $session['open_when_querying'] ?? false);

// The mechanism, from both sides. The holder went on running while the session
// request waited: its query of a second took about a second, not as long as
// the other's wait could last.
$t->assertLessThan(
    'the holder\'s one-second query was not held up by the open session',
    (float) ($hold['query_done_at'] ?? INF) - (float) ($hold['query_started_at'] ?? 0),
    1.5
);
// And the session request was answered once the holder was done, not when its
// wait gave up.
$t->assertNull('the session request\'s query was answered, not refused', $session['error'] ?? null);
$t->assertSame('with its own result', (string) ($session['result'] ?? ''), '7');
$t->assertGreaterThan(
    'after the holder was done with the connection',
    (float) ($session['query_done_at'] ?? 0),
    (float) ($hold['query_done_at'] ?? INF)
);
$t->assertLessThan(
    'and within half a second of it — well inside the two-second bound',
    (float) ($session['query_done_at'] ?? INF) - (float) ($hold['query_done_at'] ?? 0),
    0.5
);
$t->assertSame('under its own client\'s session', $session['id'] ?? null, $sessionId);

$t->done();
