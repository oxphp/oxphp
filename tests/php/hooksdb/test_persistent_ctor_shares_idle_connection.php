<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_shares_idle_connection', 'hooksdb');

// What the refusal is for, said from the other side. A pooled connection is
// refused to a constructor whose options would write to it, but only where a call
// could be in flight through it — and a claim on its own does not say that. It is
// taken at the first call through the client and given up at the end of the
// request, while the PHP object it was taken for can be dropped at any point in
// between, so "claimed by another fiber" covers a connection nothing is using and
// nothing can start using.
//
// Two ways that happens, and both must share. Refusing them would cost a
// connection per constructor for no protection at all, and the second would cost
// more than that: with nothing but the pool referencing the handle, PDO's answer
// to a dead connection takes its refcount to zero while the entry it drops is only
// blanked, leaving the socket open with nothing left that could ever close it.
//
// Both constructors below pass PDO::ATTR_AUTOCOMMIT, at the value the connection
// already has. That is the option the busy case is refused for whatever its value
// — PDO hands it to the driver, and being told is a command — so it is what tells
// this test's two answers apart from that one.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

// ── The connection this request holds itself ──────────────────────────────────
// A second handle on it is this fiber writing to a connection this fiber is not
// using, which is what any application does when it builds a handle per component
// off one pool key.
$ownKey = 'ctor-own-' . bin2hex(random_bytes(4));
$first = new PDO($dsn, $user, $pass, [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => $ownKey,
]);
// Claims the connection for this fiber; the constructor alone does not.
$firstId = $first->query('SELECT CONNECTION_ID()')->fetchColumn();

$second = new PDO($dsn, $user, $pass, [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => $ownKey,
    PDO::ATTR_AUTOCOMMIT => true,
]);
$secondId = $second->query('SELECT CONNECTION_ID()')->fetchColumn();

$t->assertSame(
    'a second handle on the connection this request already holds is the same connection',
    (string) $secondId,
    (string) $firstId
);

// ── The connection another request claims but no longer uses ──────────────────
// The holder queries, drops its handle, and parks on something that is not the
// connection. Its claim stands until its request ends; the pool is by then the
// only thing referencing the connection.
$sharedState['ctor_idle_key'] = 'ctor-idle-' . bin2hex(random_bytes(4));
unset($sharedState['ctor_idle_parked'], $sharedState['ctor_idle_released']);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_idle.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['ctor_idle_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the other request claimed the connection and let its handle go before this one woke',
    $sharedState['ctor_idle_parked'] ?? false
);

$mine = null;
$error = '';
$stillHeld = false;
try {
    $pdo = new PDO($dsn, $user, $pass, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['ctor_idle_key'],
        PDO::ATTR_AUTOCOMMIT => true,
    ]);
    // Before the query, which waits for the holder's claim and would therefore
    // always find it given up.
    $stillHeld = !($sharedState['ctor_idle_released'] ?? false);
    $mine = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

$t->assertTrue('and still claimed it when this request\'s constructor finished', $stillHeld);

$body = oxphp_async_await($task)['body'];
$t->assertSame('this request got its own answer: ' . $error, $error, '');

preg_match('/^persistent-ctor-idle-done: id:(\d+)$/m', $body, $m);
$t->assertSame(
    'and shared the claimed connection rather than opening one beside it',
    (string) $mine,
    $m[1] ?? 'the other request printed no connection id'
);

$t->done();
