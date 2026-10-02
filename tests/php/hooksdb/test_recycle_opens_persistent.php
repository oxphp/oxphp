<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// First half of a pair; hooksdb/test_recycle_closes_persistent is the second.
//
// A persistent connection belongs to the worker thread that opened it. This one
// opens one, makes sure it can be seen from the server side, records which
// connection and which worker it was, and asks the worker to retire once this
// request is answered. The next test runs on the replacement and checks that the
// retired thread took its connection with it rather than leaving it open in the
// process, where nothing could reach it again.

$t = new TestCase('recycle_opens_persistent', 'hooksdb');

$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$worker = OxPHP\Server\Worker::current();
$t->assertTrue('worker mode is on', OxPHP\Server\Worker::isWorkerMode());

// A key of its own, so the pool entry is this test's and no other test reuses it.
$key = 'recycle-probe-' . bin2hex(random_bytes(4));
$persistent = new PDO($dsn, $user, $pass, [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => $key,
]);
$id = (int) $persistent->query('SELECT CONNECTION_ID()')->fetchColumn();
$t->assertGreaterThan('the persistent connection has an id', $id, 0);

// What the second half looks for has to be visible here first, or its absence
// there would mean nothing.
$observer = new PDO($dsn, $user, $pass, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$seen = $observer->prepare('SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = ?');
$seen->execute([$id]);
$t->assertSame('the server lists the persistent connection', (string) $seen->fetchColumn(), '1');
$observer = null;

$written = file_put_contents('/tmp/oxphp-recycle-probe.json', json_encode([
    'connection_id' => $id,
    'worker_start' => $worker->startTime(),
]));
$t->assertTrue('the probe was recorded', $written !== false);

$worker->scheduleExit();
$t->assertTrue('the exit is scheduled', $worker->isExitScheduled());

$t->done();
