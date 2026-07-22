<?php

// SQLite (pdo_sqlite, in-memory — no server). Table t_sqlite keeps its spans
// distinguishable from the mysql/pgsql fixtures in the shared collector log.

$pdo = new PDO('sqlite::memory:', null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$pdo->exec('CREATE TABLE t_sqlite (id INTEGER PRIMARY KEY, email TEXT)');
$pdo->exec("INSERT INTO t_sqlite (id, email) VALUES (1, 'alice@example.com')");

// Direct query with a string literal — obfuscation must strip the email to `?`.
$pdo->query("SELECT * FROM t_sqlite WHERE email = 'alice@example.com'")->fetchAll();

// Prepared statement with a bound parameter → db.statement on the prepare span,
// db.params captured on execute.
$prep = $pdo->prepare('SELECT * FROM t_sqlite WHERE id = ?');
$prep->execute([1]);
$prep->fetchAll();

// Slow query: a recursive CTE (SQLite has no SLEEP) reliably exceeds 1ms.
$pdo->query(
    'WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 500000) '
    . 'SELECT count(*) AS n FROM c'
)->fetch();

echo "ok\n";
