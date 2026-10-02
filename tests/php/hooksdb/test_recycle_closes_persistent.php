<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Second half of hooksdb/test_recycle_opens_persistent, which opened a
// persistent connection and retired its worker.
//
// A thread's persistent connections live in a list of its own that PHP closes
// only at process shutdown, and only for threads still registered then. A
// worker that retires before that has to close them itself on the way out;
// otherwise every recycle leaves one more connection open on the database server
// until it runs out of them.

$t = new TestCase('recycle_closes_persistent', 'hooksdb');

$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$probe = json_decode((string) @file_get_contents('/tmp/oxphp-recycle-probe.json'), true);
$t->assertTrue('the first half recorded its probe', is_array($probe));
$probe = is_array($probe) ? $probe : ['connection_id' => 0, 'worker_start' => INF];
@unlink('/tmp/oxphp-recycle-probe.json');

// The premise, read now: this request runs on a worker started after the one
// that opened the connection, so that one has gone.
$t->assertGreaterThan(
    'a replacement worker serves this request',
    OxPHP\Server\Worker::current()->startTime(),
    (float) $probe['worker_start']
);

// The retired thread closes its connections after its last request, which is
// not ordered against this one starting; give it a moment rather than none.
$observer = new PDO($dsn, $user, $pass, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
$seen = $observer->prepare('SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = ?');
$deadline = microtime(true) + 5.0;
do {
    $seen->execute([(int) $probe['connection_id']]);
    $count = (string) $seen->fetchColumn();
    $seen->closeCursor();
    if ($count === '0') {
        break;
    }
    usleep(100_000);
} while (microtime(true) < $deadline);

$t->assertSame("the retired worker's persistent connection is closed", $count, '0');

$t->done();
