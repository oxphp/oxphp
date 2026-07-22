<?php

// APM database auto-instrumentation e2e fixture.
//
// Exercises the full hook pipeline against pdo_sqlite (no external DB server):
//   - PDO::__construct  → connection metadata stored from the DSN
//   - PDO::exec         → DDL/DML statement spans
//   - PDO::query        → SELECT span with obfuscated db.statement + db.system
//   - PDO::prepare + PDOStatement::execute → SQL on the prepare span, params on execute
//     from the prepare, and (capture enabled) carries db.params
//   - a slow query      → oxphp.db.slow=true once past OTEL_APM_SLOW_QUERY_MS
//
// Assertions live in tests/apm_db_spans.sh, against the collector's debug export.

$pdo = new PDO('sqlite::memory:');
$pdo->exec('CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)');
$pdo->exec("INSERT INTO users (id, email) VALUES (1, 'alice@example.com')");

// Direct query with a string literal — obfuscation must strip the email to `?`.
$stmt = $pdo->query("SELECT * FROM users WHERE email = 'alice@example.com'");
$stmt->fetchAll();

// Prepared statement with a bound parameter. The SQL is on the prepare span
// (read from its own args); the execute span records db.params when enabled.
$prep = $pdo->prepare('SELECT * FROM users WHERE id = ?');
$prep->execute([1]);
$prep->fetchAll();

// Slow query: a recursive CTE counting to 500k reliably exceeds a 1ms
// threshold, so its span is flagged oxphp.db.slow=true.
$slow = $pdo->query(
    'WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 500000) '
    . 'SELECT count(*) AS n FROM c'
);
$slow->fetch();

echo "ok\n";
