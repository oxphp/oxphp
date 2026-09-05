<?php

declare(strict_types=1);

// The holder for the driver that has no liveness check of its own. pdo_sqlite
// leaves that slot in its method table empty, and PDO calls the check only when
// there is one — so a pooled connection opened through this driver is handed to
// the next constructor with nothing asked about it at all, and that
// constructor's options are written onto it while this request is using it.
//
// A query before the flag, because a claim is taken by a call on the connection
// and not by the constructor that built it: without it this request would be the
// connection's creator rather than its holder. The table it creates is also how
// the other side tells the connections apart — an in-memory database belongs to
// the connection that opened it, so nothing but this connection can see it.
try {
    $key = $sharedState['ctor_sqlite_key'] ?? 'ctor-sqlite-fixed';

    $pdo = new PDO('sqlite::memory:', null, null, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);

    $pdo->exec('CREATE TABLE ox_holder_marker (n INTEGER)');
    $sharedState['ctor_sqlite_parked'] = true;

    // Parked on something other than the database, which is what a claim on this
    // driver looks like: sqlite has no socket to park in, and a claim is held to
    // the end of the request whatever the fiber is waiting on meanwhile.
    oxphp_usleep(1_000_000);

    // Both ends of the window, as everywhere else here: the flag above stays true
    // after this request is over, so on its own it cannot say the constructor
    // being measured ran while the connection was still held.
    $sharedState['ctor_sqlite_released'] = true;

    // The error mode is what a constructor adopting this connection changes on it
    // first: PDO writes it onto the pooled handle from its own options, defaults
    // included, and nothing tells this request it happened.
    echo 'persistent-ctor-sqlite-done: errmode:' . $pdo->getAttribute(PDO::ATTR_ERRMODE);
} catch (\Throwable $e) {
    echo 'persistent-ctor-sqlite-failed:' . str_replace("\n", ' ', $e->getMessage());
}
