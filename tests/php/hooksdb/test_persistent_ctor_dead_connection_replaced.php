<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_dead_connection_replaced', 'hooksdb');

// The other half of the liveness rule, and the one that keeps it honest: a pooled
// connection nobody is mid-exchange on is checked exactly as PDO checks it, so one
// that has really died is still dropped and replaced inside the constructor.
//
// Worth its own test because the rule in front of that check answers "alive"
// without asking the server, and a rule that answered that way in every case would
// leave the pool handing out dead connections with nothing to catch it. The way a
// pooled connection dies in production is the server closing it — wait_timeout, an
// administrator, a restart — and KILL is that, on demand.
$key = 'ctor-dead-' . bin2hex(random_bytes(4));
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';
$opts = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_PERSISTENT => $key];

$pdo = new PDO($dsn, $user, $pass, $opts);
$first = (string) $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();

// The object goes; the connection stays in the pool, which is the state a later
// request finds it in.
unset($pdo);

// Killed from a connection of its own, so nothing about this request is what ends
// it — the same way the server would end it on its own.
$killer = new PDO($dsn, $user, $pass, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$killer->exec('KILL ' . (int) $first);
unset($killer);
oxphp_usleep(200_000);

$second = '';
$error = '';
try {
    $again = new PDO($dsn, $user, $pass, $opts);
    $second = (string) $again->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

// Handing the dead one over is what this catches: the query on it is the first
// thing that would notice, and it would not answer at all.
$t->assertSame('the constructor returned a usable connection: ' . $error, $error, '');
$t->assertNotEqual(
    'and it replaced the connection that had died rather than handing it back',
    $second,
    $first
);

$t->done();
