<?php

declare(strict_types=1);

// The holder for the driver whose end-of-request cleanup is the easiest to see:
// pdo_sqlite's unregisters every SQL function registered on the connection. PDO
// runs that cleanup from an object's destructor, before it looks at whether other
// objects still share the pooled connection — so another request dropping its
// own handle on this connection takes this request's functions away from it.
//
// A statement before the flag, because a claim is taken by a call on the
// connection and not by the constructor that built it.
try {
    $key = $sharedState['dtor_sqlite_key'] ?? 'dtor-sqlite-fixed';
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ];

    // PDO::connect() for the driver's own class, whose createFunction() is the
    // spelling both supported PHP versions accept without a deprecation.
    $pdo = PDO::connect('sqlite::memory:', null, null, $opts);
    $pdo->createFunction('ox_dtor_f', fn ($x) => $x * 2, 1);
    $pdo->exec('CREATE TABLE ox_dtor_marker (n INTEGER)');
    $sharedState['dtor_sqlite_parked'] = true;

    $deadline = microtime(true) + 5.0;
    while (!($sharedState['dtor_sqlite_dropped'] ?? false) && microtime(true) < $deadline) {
        oxphp_usleep(20_000);
    }
    $sharedState['dtor_sqlite_released'] = true;

    $before = $pdo->query('SELECT ox_dtor_f(21)')->fetchColumn();

    // And this request dropping its own handle still cleans up, as PDO always
    // has: the rule keeps the cleanup away from a connection another request
    // holds, not from the one holding it.
    unset($pdo);
    $again = PDO::connect('sqlite::memory:', null, null, $opts);
    $marker = $again->query("SELECT count(*) FROM sqlite_master WHERE name = 'ox_dtor_marker'")
        ->fetchColumn();
    try {
        $after = 'still-registered:' . $again->query('SELECT ox_dtor_f(21)')->fetchColumn();
    } catch (\PDOException $e) {
        $after = str_contains($e->getMessage(), 'no such function') ? 'unregistered' : $e->getMessage();
    }

    echo 'persistent-dtor-sqlite-done: before:' . $before . ' marker:' . $marker . ' after:' . $after;
} catch (\Throwable $e) {
    echo 'persistent-dtor-sqlite-failed:' . str_replace("\n", ' ', $e->getMessage());
}
