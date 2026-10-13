<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('session_slot_claim_cycle_breaks', 'hooksdb');

// A request with its session open that needs a connection held by a request
// waiting for that very session does not wait for the connection: neither could
// ever go on, so the wait is given up at once rather than when its bound runs
// out.
//
// One session at a time is open on a worker, so session_start() waits while
// another request on it has its session open. A claim on a shared connection
// lasts until the request that took it ends. A request that took the connection
// and then calls session_start(), beside one that opened its session and then
// queries that connection, waits for the second while the second waits for it.
//
// The first request takes the connection with a short query and parks. The
// second opens its session and queries the connection, and parks waiting for it.
// The first wakes and calls session_start(). The second's wait is then given up
// — with the outcome of a wait that runs out, which for PDO hands the call to
// the client — and since nothing is in the middle of an exchange on the
// connection by then, the client runs the query. Then its session closes, and
// the first gets the session slot.

$sharedState['pdo_session_cycle'] ??= new PDO(
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
    fwrite($sock, "GET /tests/hooksdb/fixture_session_claim.php?key=pdo_session_cycle&$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\n"
        . "Cookie: PHPSESSID=$cookie\r\n"
        . "Connection: close\r\n\r\n");

    return $sock;
};
$read = static function ($sock): mixed {
    $raw = (string) stream_get_contents($sock);
    fclose($sock);
    $split = strpos($raw, "\r\n\r\n");

    return json_decode($split === false ? $raw : substr($raw, $split + 4), true);
};

$claimId = str_repeat('c2', 16);
$sessionId = str_repeat('d3', 16);

unset($sharedState['session_claim_holding']);
$claimSock = $send('role=claim_then_session&park=0.3', $claimId);
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['session_claim_holding']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$sessionSock = $send('role=session', $sessionId);

$session = $read($sessionSock);
$claim = $read($claimSock);

$t->meta('claim', $claim);
$t->meta('session', $session);

$t->assertTrue('the claiming request answered with JSON', is_array($claim));
$t->assertTrue('the session request answered with JSON', is_array($session));
if (!is_array($claim) || !is_array($session)) {
    $t->done();
}

$t->assertSame('the claiming request ran its query', (string) ($claim['result'] ?? ''), '3');

// The premise: each was waiting for the other. The session request was waiting
// for the connection when the claiming one began waiting for the session.
$t->assertLessThan(
    'the session request queried before the claiming one began waiting for the session',
    (float) ($session['query_started_at'] ?? INF),
    (float) ($claim['wait_started_at'] ?? 0)
);
$t->assertGreaterThan(
    'and was still waiting for the connection when it did',
    (float) ($session['query_done_at'] ?? 0),
    (float) ($claim['wait_started_at'] ?? INF)
);

// The mechanism: the session request's wait was given up at once, not when its
// two-second bound ran out.
$t->assertNull('the session request\'s query was answered', $session['error'] ?? null);
$t->assertSame('with its own result', (string) ($session['result'] ?? ''), '7');
$t->assertLessThan(
    'within half a second of the claiming request beginning to wait for the session',
    (float) ($session['query_done_at'] ?? INF) - (float) ($claim['wait_started_at'] ?? 0),
    0.5
);
$t->assertLessThan(
    'so its query took well under the bound — the other\'s park, and not the two seconds',
    (float) ($session['query_done_at'] ?? INF) - (float) ($session['query_started_at'] ?? 0),
    1.0
);

// And the claiming request got the session slot once the other's closed.
$t->assertGreaterThan(
    'the claiming request\'s session_start() returned only once the other session had closed',
    (float) ($claim['started_at'] ?? 0),
    (float) ($session['closed_at'] ?? INF)
);
$t->assertSame('under its own client\'s session', $claim['id'] ?? null, $claimId);
$t->assertSame('with nothing of the other\'s in it', $claim['who'] ?? null, 'none');

$t->done();
