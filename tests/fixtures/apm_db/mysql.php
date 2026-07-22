<?php

// MySQL via PDO (pdo_mysql). Connection metadata comes from parsing the DSN.

$host = getenv('DB_MYSQL_HOST') ?: 'mysql';
$db   = getenv('DB_NAME') ?: 'appdb';
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$pdo = new PDO(
    "mysql:host=$host;port=3306;dbname=$db",
    $user,
    $pass,
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);
$pdo->exec('CREATE TABLE IF NOT EXISTS t_mysql (id INT PRIMARY KEY, email VARCHAR(255))');
$pdo->exec("INSERT INTO t_mysql (id, email) VALUES (1, 'alice@example.com') ON DUPLICATE KEY UPDATE email = email");

$pdo->query("SELECT * FROM t_mysql WHERE email = 'alice@example.com'")->fetchAll();

$prep = $pdo->prepare('SELECT * FROM t_mysql WHERE id = ?');
$prep->execute([1]);
$prep->fetchAll();

// Slow query — MySQL SLEEP(0.05) reliably exceeds a 1ms threshold.
$pdo->query('SELECT SLEEP(0.05)')->fetch();

echo "ok\n";
