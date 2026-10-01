<?php

declare(strict_types=1);

// Builds, or drops, a PDO handle kept in a static property, so it outlives the
// request that built it — the way a worker-mode application keeps one in a
// service container — and another request can be the one to let go of it.
//
//   ?step=build&key=K  build a handle on persistent key K, keep it, and say which
//                      fiber this request ran on;
//   ?step=drop&key=K   open a transaction on K through a handle of this request's
//                      own, drop the kept handle — built by an earlier request and
//                      never used since — and report whether the transaction
//                      survived.
if (!class_exists('OxDtorUserBox', false)) {
    final class OxDtorUserBox
    {
        public static ?PDO $pdo = null;
    }
}

$fiber = spl_object_id(Fiber::getCurrent());

try {
    $step = $_GET['step'] ?? '';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
    $opts = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_PERSISTENT => (string) ($_GET['key'] ?? '')];

    if ($step === 'build') {
        OxDtorUserBox::$pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        echo 'persistent-dtor-user-built: fiber:' . $fiber;
    } elseif ($step === 'drop') {
        $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        $pdo->exec('CREATE TEMPORARY TABLE ox_dtor_idle (n INT) ENGINE=InnoDB');
        $pdo->beginTransaction();
        $pdo->exec('INSERT INTO ox_dtor_idle VALUES (1)');

        $had = OxDtorUserBox::$pdo !== null;
        OxDtorUserBox::$pdo = null;

        $inTxn = $pdo->inTransaction();
        $pdo->commit();
        $rows = $pdo->query('SELECT COUNT(*) FROM ox_dtor_idle')->fetchColumn();
        echo 'persistent-dtor-user-dropped: had:' . var_export($had, true) . ' fiber:' . $fiber
            . ' in_txn:' . var_export($inTxn, true) . ' rows:' . $rows;
    }
} catch (\Throwable $e) {
    echo 'persistent-dtor-user-failed: fiber:' . $fiber . ' ' . str_replace("\n", ' ', $e->getMessage());
}
