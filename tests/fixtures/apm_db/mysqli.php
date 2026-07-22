<?php

// MySQL via the mysqli OO API — the least-covered hook path. Connection metadata
// is built from the constructor's positional arguments (host, user, pass, db,
// port), not a DSN. mysqli binds parameters out-of-band via bind_param(), so the
// execute() call carries no params array — db.params is not captured here (that
// is a PDO-only capability). The prepared statement's SQL is on the
// mysqli::prepare span (read from its own args), not the execute span.

$host = getenv('DB_MYSQL_HOST') ?: 'mysql';
$db   = getenv('DB_NAME') ?: 'appdb';
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

mysqli_report(MYSQLI_REPORT_ERROR | MYSQLI_REPORT_STRICT);
$m = new mysqli($host, $user, $pass, $db, 3306);

$m->query('CREATE TABLE IF NOT EXISTS t_mysqli (id INT PRIMARY KEY, email VARCHAR(255))');
$m->query("INSERT INTO t_mysqli (id, email) VALUES (1, 'alice@example.com') ON DUPLICATE KEY UPDATE email = email");

$m->query("SELECT * FROM t_mysqli WHERE email = 'alice@example.com'")->fetch_all();

$stmt = $m->prepare('SELECT * FROM t_mysqli WHERE id = ?');
$id = 1;
$stmt->bind_param('i', $id);
$stmt->execute();
$stmt->get_result();

// Slow query — SLEEP(0.05) via mysqli::query.
$m->query('SELECT SLEEP(0.05)');

echo "ok\n";
