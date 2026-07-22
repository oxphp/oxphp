<?php

// Regression for PHP object-handle reuse (was a real bug): a freed prepared
// statement must never leak its SQL onto an unrelated statement that recycles
// its object handle. db.statement is read from each call's own arguments
// (query/prepare) and from the statement object's own queryString (execute) —
// never from a handle-keyed store — so the sensitive statement ($s1, selecting
// `ssn`) can appear only on its own spans and the benign one ($s2, selecting
// `id`) only on its own. The test asserts the two counts are equal: a leak of
// $s1's SQL onto $s2's recycled-handle execute would make `ssn` outnumber `id`.

$pdo = new PDO('sqlite::memory:', null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$pdo->exec('CREATE TABLE secrets (id INTEGER PRIMARY KEY, ssn TEXT)');
$pdo->exec("INSERT INTO secrets (id, ssn) VALUES (1, '111-22-3333')");

// Prepare + execute a statement selecting a sensitive column, then free it so
// PHP can recycle its object handle from the free list.
$s1 = $pdo->prepare('SELECT ssn FROM secrets WHERE id = ?');
$s1->execute([1]);
$s1->fetchAll();
unset($s1);

// A fresh statement — likely reusing the freed handle. Under the old handle-
// keyed store its execute would have read $s1's stale SQL; now it cannot.
$s2 = $pdo->prepare('SELECT id FROM secrets WHERE id = ?');
$s2->execute([1]);
$s2->fetchAll();

echo "ok\n";
