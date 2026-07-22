<?php

// PostgreSQL via PDO (pdo_pgsql). Connection metadata comes from parsing the DSN
// (driver `pgsql` maps to db.system `postgresql`).

$host = getenv('DB_PG_HOST') ?: 'postgres';
$db   = getenv('DB_NAME') ?: 'appdb';
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$pdo = new PDO(
    "pgsql:host=$host;port=5432;dbname=$db",
    $user,
    $pass,
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);
$pdo->exec('CREATE TABLE IF NOT EXISTS t_pg (id INT PRIMARY KEY, email TEXT)');
$pdo->exec("INSERT INTO t_pg (id, email) VALUES (1, 'alice@example.com') ON CONFLICT (id) DO NOTHING");

$pdo->query("SELECT * FROM t_pg WHERE email = 'alice@example.com'")->fetchAll();

$prep = $pdo->prepare('SELECT * FROM t_pg WHERE id = ?');
$prep->execute([1]);
$prep->fetchAll();

// Slow query — pg_sleep(0.05) reliably exceeds a 1ms threshold.
$pdo->query('SELECT pg_sleep(0.05)')->fetch();

echo "ok\n";
