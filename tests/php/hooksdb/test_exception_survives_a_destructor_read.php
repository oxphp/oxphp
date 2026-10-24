<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('exception_survives_a_destructor_read', 'hooksdb');

// An exception thrown out of a function frees that function's locals on its way
// out, and the destructor of a PDO object with a transaction still open sends a
// ROLLBACK and reads the server's reply. That read reaches the hooked socket read
// with the exception still pending. Zend keeps no exception across a fiber
// switch, and the scheduler clears whatever it finds pending once a fiber has
// parked, so a read that parked there came back with the exception gone: the
// unwind stopped where it was, and the caller went on from the line after the
// call as though it had returned. The catch below never ran.
//
// Each case throws out of a closure whose local holds the object, and reports
// whether the caller's catch ran with that exception and whether the line after
// the call did; one more does the same with exit(). The last one asks what
// becomes of the wait itself: it is still a suspension, so the worker serves
// another request meanwhile.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';
$plain = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION];
$unbuffered = $plain + [Pdo\Mysql::ATTR_USE_BUFFERED_QUERY => false];
$seen = static fn (\Throwable $e): string => $e->getMessage() === 'boom'
    ? 'yes'
    : get_class($e) . ': ' . $e->getMessage();

// No transaction: the destructor reads nothing, so this shape delivers the
// exception whatever the hook does. What the cases below add is the read.
$caught = 'no';
$after = 'not-reached';
try {
    (static function () use ($dsn, $user, $pass, $plain): void {
        $pdo = new PDO($dsn, $user, $pass, $plain);
        $pdo->exec('DO 1');
        throw new \RuntimeException('boom');
    })();
    $after = 'reached';
} catch (\RuntimeException $e) {
    $caught = $seen($e);
}
$t->assertSame('without a read in the destructor the catch runs', "{$caught} {$after}", 'yes not-reached');

// A transaction left open: the destructor rolls it back.
$caught = 'no';
$after = 'not-reached';
try {
    (static function () use ($dsn, $user, $pass, $plain): void {
        $pdo = new PDO($dsn, $user, $pass, $plain);
        $pdo->beginTransaction();
        $pdo->exec('DO 1');
        throw new \RuntimeException('boom');
    })();
    $after = 'reached';
} catch (\RuntimeException $e) {
    $caught = $seen($e);
}
$t->assertSame('a rollback read in the destructor keeps the exception', "{$caught} {$after}", 'yes not-reached');

// The same on a persistent connection, which outlives the object, so what the
// rollback left behind can be asked afterwards. Keeping the exception is half of
// it; the other half is that the reply was read: a read that gave up instead
// would leave mysqlnd with a connection it can no longer use. The temporary
// table exists only on the connection that made it, so finding it, empty, says
// the connection is the same one and that the rollback took effect.
$popts = $plain + [PDO::ATTR_PERSISTENT => 'exc-dtor-read-' . bin2hex(random_bytes(4))];
$connId = 0;
$caught = 'no';
$after = 'not-reached';
try {
    (static function () use ($dsn, $user, $pass, $popts, &$connId): void {
        $pdo = new PDO($dsn, $user, $pass, $popts);
        $connId = (int) $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
        $pdo->exec('CREATE TEMPORARY TABLE exc_dtor_read (n INT) ENGINE=InnoDB');
        $pdo->beginTransaction();
        $pdo->exec('INSERT INTO exc_dtor_read VALUES (1)');
        throw new \RuntimeException('boom');
    })();
    $after = 'reached';
} catch (\RuntimeException $e) {
    $caught = $seen($e);
}
$t->assertSame('a rollback read on a persistent connection keeps the exception', "{$caught} {$after}", 'yes not-reached');

$state = '';
try {
    $again = new PDO($dsn, $user, $pass, $popts);
    $againId = (int) $again->query('SELECT CONNECTION_ID()')->fetchColumn();
    $rows = (int) $again->query('SELECT COUNT(*) FROM exc_dtor_read')->fetchColumn();
    $state = sprintf(
        'same_conn:%s rows:%d in_txn:%s',
        $againId === $connId ? 'yes' : "no({$connId}->{$againId})",
        $rows,
        $again->inTransaction() ? 'yes' : 'no'
    );
    $again->exec('DROP TEMPORARY TABLE exc_dtor_read');
} catch (\Throwable $e) {
    $state = 'error: ' . str_replace("\n", ' ', $e->getMessage());
}
$t->assertSame('and the rollback was read to its end on the same connection', $state, 'same_conn:yes rows:0 in_txn:no');

// An unbuffered result left half read: freeing the statement reads the rest of
// the rows off the wire. A result of about 2 MB is far more than one read takes.
$caught = 'no';
$after = 'not-reached';
try {
    (static function () use ($dsn, $user, $pass, $unbuffered): void {
        $pdo = new PDO($dsn, $user, $pass, $unbuffered);
        $stmt = $pdo->query(
            "WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 1000) "
            . "SELECT n, REPEAT('x', 2000) FROM s"
        );
        $stmt->fetch();
        throw new \RuntimeException('boom');
    })();
    $after = 'reached';
} catch (\RuntimeException $e) {
    $caught = $seen($e);
}
$t->assertSame('an unbuffered result read in the destructor keeps the exception', "{$caught} {$after}", 'yes not-reached');

// The open transaction again, inside an oxphp_async() task, whose fiber is
// resumed by a scheduler of its own.
$task = oxphp_async(static function () use ($dsn, $user, $pass, $plain): string {
    $caught = 'no';
    $after = 'not-reached';
    try {
        (static function () use ($dsn, $user, $pass, $plain): void {
            $pdo = new PDO($dsn, $user, $pass, $plain);
            $pdo->beginTransaction();
            $pdo->exec('DO 1');
            throw new \RuntimeException('boom');
        })();
        $after = 'reached';
    } catch (\RuntimeException $e) {
        $caught = $e->getMessage() === 'boom' ? 'yes' : get_class($e) . ': ' . $e->getMessage();
    }

    return "{$caught} {$after}";
});
$t->assertSame('a rollback read in a task keeps the exception', oxphp_async_await($task, 10.0), 'yes not-reached');

// exit() unwinds the same way, and the same read lost it: the request went on
// past the exit(). It ends the request it is called in, so another request runs
// it; this one waits for that reply in a hooked read, and the worker serves the
// other meanwhile.
$reply = file_get_contents(
    'http://127.0.0.1/tests/hooksdb/fixture_exit_past_a_destructor_read.php',
    false,
    stream_context_create(['http' => ['timeout' => 10]])
);
$t->assertSame('a rollback read in the destructor does not lose an exit()', $reply, 'exiting;');

// The wait is still a suspension. The server makes the rows below half a second
// apart, each larger than its network buffer, so each is on the wire as soon as
// it is made: the first fetch returns after half a second, and the rest of the
// result takes two more. While the statement's destructor reads it off, a task —
// on a thread of its own — sends a second request to the server. PHP_WORKERS=1,
// so only this request's thread can serve it, and it is served before the catch
// runs only if the read gave the thread up.
$probe = oxphp_async(static function (): string {
    oxphp_usleep(1_200_000);
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_served_at.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return $body;
});
$thrownAt = 0.0;
$caughtAt = 0.0;
$caught = 'no';
$after = 'not-reached';
try {
    (static function () use ($dsn, $user, $pass, $unbuffered, &$thrownAt): void {
        $pdo = new PDO($dsn, $user, $pass, $unbuffered);
        $stmt = $pdo->query(
            "SELECT n, SLEEP(0.5), REPEAT('x', 40000) FROM "
            . '(SELECT 1 n UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5) s'
        );
        $stmt->fetch();
        $thrownAt = microtime(true);
        throw new \RuntimeException('boom');
    })();
    $after = 'reached';
} catch (\RuntimeException $e) {
    $caughtAt = microtime(true);
    $caught = $seen($e);
}
$body = oxphp_async_await($probe, 10.0);
$servedAt = preg_match('/served-at:([0-9.]+)/', $body, $m) === 1 ? (float) $m[1] : 0.0;
$t->assertSame('a slow unbuffered result read in the destructor keeps the exception', "{$caught} {$after}", 'yes not-reached');
$t->assertGreaterThan('the destructor really waited for the rest of the result', $caughtAt - $thrownAt, 1.0);
$t->assertSame(
    'the worker served another request while it waited',
    $servedAt > $thrownAt && $servedAt < $caughtAt
        ? 'during'
        : sprintf(
            'outside (thrown %.3f, served %.3f, caught %.3f, reply %s)',
            $thrownAt,
            $servedAt,
            $caughtAt,
            json_encode(substr($body, 0, 120))
        ),
    'during'
);

$t->done();
