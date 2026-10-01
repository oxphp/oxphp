<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_follows_last_user', 'hooksdb');

// Which request a PDO handle belongs to, for what dropping it does to a pooled
// connection, is the one that last used it — not the one that built it. Both
// halves below drop a handle an earlier request built and kept in a static
// property, from a request holding the connection, and differ only in whether
// that request used the handle first.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$fetch = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_user.php?{$query} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return $body;
};

// 1. Built by an earlier request, then used by this one to open a transaction.
// Dropping it is this request dropping its own handle, and rolls the transaction
// back as PDO always has.
$usedKey = 'dtor-user-used-' . bin2hex(random_bytes(4));
$t->assertContains('an earlier request built the handle', $fetch('step=build&key=' . $usedKey), 'persistent-dtor-user-built:');

$handed = OxDtorUserBox::$pdo;
OxDtorUserBox::$pdo = null;
$handed->exec('CREATE TEMPORARY TABLE ox_dtor_user (n INT) ENGINE=InnoDB');
$handed->beginTransaction();
$handed->exec('INSERT INTO ox_dtor_user VALUES (1)');
unset($handed);

$again = new PDO($dsn, $user, $pass, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_PERSISTENT => $usedKey]);
$t->assertFalse(
    'a handle this request used rolls its transaction back when dropped, whoever built it',
    $again->inTransaction()
);
$t->assertSame(
    'and the row it inserted is gone',
    (string) $again->query('SELECT COUNT(*) FROM ox_dtor_user')->fetchColumn(),
    '0'
);
unset($again);

// 2. Built by an earlier request and never used again, then dropped by a later
// request from inside a transaction of its own. The handle is the earlier
// request's, so the transaction stays. The later request runs on the very fiber
// the earlier one ran on — fibers are reused — which is what makes this a case:
// the same fiber, but not the same request. The fiber is compared by the id of
// its Fiber object, which a freed object's successor could also take, so equal
// ids are a strong hint rather than proof; dropping the request half of the
// match (comparing the fiber alone) is what turns this case red.
$idleKey = 'dtor-user-idle-' . bin2hex(random_bytes(4));
$built = $fetch('step=build&key=' . $idleKey);
$dropped = $fetch('step=drop&key=' . $idleKey);
preg_match('/built: fiber:(\d+)/', $built, $b);
preg_match('/ fiber:(\d+) /', $dropped, $d);

$t->assertContains('the later request found the kept handle', $dropped, 'had:true');
$t->assertSame('and ran on the fiber the earlier one had run on', $d[1] ?? 'none', $b[1] ?? 'unbuilt');
$t->assertContains(
    'and dropping the earlier request\'s handle left its own transaction open',
    $dropped,
    'in_txn:true rows:1'
);

$t->done();
