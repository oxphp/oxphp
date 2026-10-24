<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_leaves_transaction_to_holders_other_handle', 'hooksdb');

// The case PHP 8.4.27 and 8.5.12 changed PDO for: a script with two handles on
// one pooled connection drops one of them mid-transaction and goes on with the
// transaction through the other. Those releases leave the transaction open for
// it; earlier ones roll it back. Rolling back a handle's transaction beside
// another request's handles must not take this away from a request whose other
// handle is its own.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$opts = [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => 'dtor-pair-' . bin2hex(random_bytes(4)),
];
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$first = new PDO($dsn, $user, $pass, $opts);
$second = new PDO($dsn, $user, $pass, $opts);
$first->exec('CREATE TEMPORARY TABLE ox_dtor_pair (n INT) ENGINE=InnoDB');
$first->beginTransaction();
$first->exec('INSERT INTO ox_dtor_pair VALUES (1)');
unset($first);

$inTxn = $second->inTransaction();
if ($inTxn) {
    $second->commit();
}
$rows = (string) $second->query('SELECT COUNT(*) FROM ox_dtor_pair')->fetchColumn();

$keeps = PHP_VERSION_ID >= 80512 || (PHP_VERSION_ID >= 80427 && PHP_VERSION_ID < 80500);
$t->assertSame(
    $keeps
        ? 'the transaction stayed open for the handle this request still has'
        : 'this PHP release rolls the transaction back with the dropped handle',
    $inTxn,
    $keeps
);
$t->assertSame('and the row is there only if it could be committed', $rows, $keeps ? '1' : '0');

$t->done();
