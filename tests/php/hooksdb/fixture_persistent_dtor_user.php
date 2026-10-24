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
//                      survived;
//   ?step=drop-own&key=K&aside=K2&how=unset|throw
//                      with a kept handle on K built by an earlier request and a
//                      handle of this request's own on K2, open a transaction on
//                      K through a handle of its own and drop that handle — by
//                      unset(), or by an exception unwinding the frame holding it
//                      — then report whether the transaction survived;
//   ?step=open&key=K   build two handles on K, keep both, and end the request
//                      with a transaction open through one of them.
if (!class_exists('OxDtorUserBox', false)) {
    final class OxDtorUserBox
    {
        public static ?PDO $pdo = null;
        public static ?PDO $other = null;
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
    } elseif ($step === 'drop-own') {
        $aside = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_PERSISTENT => (string) ($_GET['aside'] ?? ''),
        ]);
        $had = OxDtorUserBox::$pdo !== null;

        $conn = '';
        $threw = false;
        $own = static function () use ($dsn, $opts, &$conn, &$threw): void {
            $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
            $pdo->exec('CREATE TEMPORARY TABLE ox_dtor_own (n INT) ENGINE=InnoDB');
            $pdo->beginTransaction();
            $pdo->exec('INSERT INTO ox_dtor_own VALUES (1)');
            $conn = (string) $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
            if (($_GET['how'] ?? '') === 'throw') {
                $threw = true;
                throw new \RuntimeException('dropped by an exception');
            }
            unset($pdo);
        };
        $caught = false;
        try {
            $own();
        } catch (\RuntimeException $e) {
            $caught = true;
        }

        $check = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        $inTxn = $check->inTransaction();
        $rows = $check->query('SELECT COUNT(*) FROM ox_dtor_own')->fetchColumn();
        $after = $check->query('SELECT CONNECTION_ID()')->fetchColumn();
        if ($inTxn) {
            $check->rollBack();
        }
        echo 'persistent-dtor-user-dropped-own: had:' . var_export($had, true) . ' fiber:' . $fiber
            . ' threw:' . var_export($threw, true) . ' caught:' . var_export($caught, true)
            . ' conn:' . $conn . '/' . $after
            . ' in_txn:' . var_export($inTxn, true) . ' rows:' . $rows;
    } elseif ($step === 'open') {
        OxDtorUserBox::$pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        OxDtorUserBox::$other = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        OxDtorUserBox::$pdo->exec('CREATE TEMPORARY TABLE ox_dtor_open (n INT) ENGINE=InnoDB');
        OxDtorUserBox::$pdo->beginTransaction();
        OxDtorUserBox::$pdo->exec('INSERT INTO ox_dtor_open VALUES (1)');
        $conn = OxDtorUserBox::$pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
        echo 'persistent-dtor-user-opened: fiber:' . $fiber . ' conn:' . $conn;
    }
} catch (\Throwable $e) {
    echo 'persistent-dtor-user-failed: fiber:' . $fiber . ' ' . str_replace("\n", ' ', $e->getMessage());
}
