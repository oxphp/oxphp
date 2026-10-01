<?php

declare(strict_types=1);

// The holder for the two cases where the cycle collector, not the code that let go
// of it, destroys a PDO object on a pooled connection this request is in the middle
// of a transaction on. The collector frees garbage on whichever fiber happens to
// run it, so whose object it is and where it is destroyed are two different
// questions:
//
//   ?mode=collect  another request leaves its handle in a garbage cycle, and this
//                  request runs the collector — the other request's object is
//                  destroyed here, and must leave this transaction alone;
//   ?mode=orphan   this request leaves its own handle in a garbage cycle, and the
//                  other request runs the collector — this request's object is
//                  destroyed there, and must still roll its transaction back, or
//                  the transaction stays open on the pooled connection.
//
// The cycle carries a sentinel whose destructor records who was collecting when it
// ran, so each side can check the collection happened where the case needs it.
try {
    $mode = ($_GET['mode'] ?? '') === 'orphan' ? 'orphan' : 'collect';
    $key = $sharedState['dtor_gc_key'] ?? 'dtor-gc-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
    $opts = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_PERSISTENT => $key];

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
    // Private to this connection, so nothing another test does can add to it.
    $pdo->exec('CREATE TEMPORARY TABLE ox_dtor_gc (n INT) ENGINE=InnoDB');
    $pdo->beginTransaction();
    $pdo->exec('INSERT INTO ox_dtor_gc VALUES (1)');

    if ($mode === 'orphan') {
        $cycle = new stdClass();
        $cycle->self = $cycle;
        $cycle->pdo = $pdo;
        $cycle->sentinel = new class ($sharedState) {
            private array $state;

            public function __construct(array &$state)
            {
                $this->state = &$state;
            }

            public function __destruct()
            {
                $this->state['dtor_gc_sentinel'] = $this->state['dtor_gc_collector'] ?? 'none';
            }
        };
        // Only the cycle holds the handle now.
        unset($pdo, $cycle);
    }

    $sharedState['dtor_gc_parked'] = true;

    $deadline = microtime(true) + 5.0;
    while (!($sharedState['dtor_gc_dropped'] ?? false) && microtime(true) < $deadline) {
        oxphp_usleep(20_000);
    }

    if ($mode === 'collect') {
        $sharedState['dtor_gc_collector'] = 'holder';
        gc_collect_cycles();
        unset($sharedState['dtor_gc_collector']);
        $collectedBy = $sharedState['dtor_gc_sentinel'] ?? 'nobody';
    }

    $sharedState['dtor_gc_released'] = true;

    if ($mode === 'collect') {
        $inTxn = $pdo->inTransaction();
        $pdo->commit();
        $rows = $pdo->query('SELECT COUNT(*) FROM ox_dtor_gc')->fetchColumn();
        echo 'persistent-dtor-gc-collect-done: collected_by:' . $collectedBy
            . ' in_txn:' . var_export($inTxn, true) . ' rows:' . $rows . ' id:' . $id;
    } else {
        // This request's only handle is gone, so a fresh one is how it asks. Its
        // transaction ended with that handle, wherever the handle was destroyed.
        $again = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        $inTxn = $again->inTransaction();
        $rows = $again->query('SELECT COUNT(*) FROM ox_dtor_gc')->fetchColumn();
        echo 'persistent-dtor-gc-orphan-done: in_txn:' . var_export($inTxn, true)
            . ' rows:' . $rows . ' id:' . $id;
    }
} catch (\Throwable $e) {
    echo 'persistent-dtor-gc-failed:' . str_replace("\n", ' ', $e->getMessage());
}
