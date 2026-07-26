<?php

declare(strict_types=1);

// Served in its own request fiber. The query takes a second on the server, and
// the wait for its answer is what the socket hooks turn into a suspension.
$pdo = new PDO(
    'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
    getenv('DB_USER') ?: 'appuser',
    getenv('DB_PASS') ?: 'apppass',
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);
$pdo->query('SELECT SLEEP(1)')->fetchColumn();

echo 'db-done';
