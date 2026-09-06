<?php

declare(strict_types=1);

// The holder for the case where the request that owns a pooled connection builds
// a second handle on it — an application reaching one pool key from two
// components does this, and so does anything that calls its connection factory
// twice — and then waits on something that is not that connection.
//
// The order is the whole point of this fixture, and it is the reverse of the one
// two objects are usually built in. The query comes first, so the connection is
// this request's before the second constructor runs; the second constructor is
// the only call that asks PDO's liveness check about a connection the asking
// request already holds; and the wait after it is on a timer rather than on the
// connection, so nothing takes the connection again on this request's behalf
// before the request on the other side arrives. What that request meets is
// therefore whatever the second constructor left behind.
try {
    $key = $sharedState['ctor_nested_key'] ?? 'ctor-nested-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ];

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();

    // Same pool key, so the same connection — the one this request is now holding.
    $second = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);

    $sharedState['ctor_nested_parked'] = true;
    oxphp_usleep(1_000_000);
    $sharedState['ctor_nested_released'] = true;

    // The premise, asked here rather than before the wait: a query on the second
    // handle would take the connection again on this request's behalf, which is
    // exactly what the window above is measuring the absence of. Asked after it,
    // it still says what it has to say — whether the second constructor reached
    // the pooled connection at all or quietly opened one beside it, which is the
    // reading under which every assertion below would hold for no reason.
    $secondId = $second->query('SELECT CONNECTION_ID()')->fetchColumn();

    // Both handles report the error mode, because a constructor that adopted this
    // connection would have written its own onto the handle they share.
    echo 'persistent-ctor-nested-done: id:' . $id . ' second-id:' . $secondId
        . ' errmode:' . $pdo->getAttribute(PDO::ATTR_ERRMODE)
        . ' second:' . $second->getAttribute(PDO::ATTR_ERRMODE);
} catch (\Throwable $e) {
    echo 'persistent-ctor-nested-failed:' . str_replace("\n", ' ', $e->getMessage());
}
