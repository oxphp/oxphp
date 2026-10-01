<?php

declare(strict_types=1);

// The holder for the destructor case where it costs memory rather than state:
// this request reads a result set whose rows arrive one at a time, so it parks
// inside the client's result-reading loop with the result half built. The
// driver's end-of-request cleanup, run from another request's destructor on the
// same pooled connection, frees exactly that result — and this request resumes
// writing rows into it.
//
// Each row is larger than the server's network buffer, so the server sends it
// as soon as it is produced instead of batching the result; the SLEEP spaces the
// rows out, which is what keeps this request inside the loop for long enough.
try {
    $key = $sharedState['dtor_stream_key'] ?? 'dtor-stream-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);
    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();

    // Both ends of the read are published, so the other side can say its handle
    // was dropped inside it rather than before or after.
    $sharedState['dtor_stream_start'] = microtime(true);
    $rows = $pdo->query(
        'WITH RECURSIVE seq (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 40) '
        . "SELECT n, REPEAT('x', 20000) AS p, SLEEP(0.05) AS s FROM seq"
    )->fetchAll(PDO::FETCH_NUM);
    $sharedState['dtor_stream_end'] = microtime(true);

    $intact = count($rows) === 40;
    foreach ($rows as $i => $row) {
        if ((int) $row[0] !== $i + 1 || $row[1] !== str_repeat('x', 20000)) {
            $intact = false;
            break;
        }
    }

    echo 'persistent-dtor-stream-done: rows:' . count($rows)
        . ' intact:' . var_export($intact, true) . ' id:' . $id;
} catch (\Throwable $e) {
    echo 'persistent-dtor-stream-failed:' . str_replace("\n", ' ', $e->getMessage());
}
