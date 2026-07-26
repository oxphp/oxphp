<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('mysql_task_multiplex', 'hooksdb');

// ASYNC_WORKERS=1: three tasks each open their own MySQL connection and run a
// query the server takes a second to answer. mysqlnd waits for that answer in
// php_stream_read() on a tcp:// stream, so a hooked read suspends the task
// fiber and the single async worker thread interleaves all three waits.
// Without the hook the thread sits in recv() and the three queries serialize.
//
// MySQL runs each connection on its own thread, so the three SLEEP(1) calls
// overlap server-side either way — the only variable this test measures is
// whether the client-side wait releases the worker thread.
$host = getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql';
$name = getenv('DB_NAME') ?: 'appdb';
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$tasks = [];
for ($i = 0; $i < 3; $i++) {
    $tasks[] = oxphp_async(function () use ($host, $name, $user, $pass): array {
        $pdo = new PDO(
            "mysql:host={$host};port=3306;dbname={$name}",
            $user,
            $pass,
            [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
        );

        // Connecting stays outside the measurement: connect(2) and the DNS
        // lookup are not hooked, and folding unhooked work into the number
        // would blunt exactly the comparison this test makes.
        $started = microtime(true);
        $slept = $pdo->query('SELECT SLEEP(1)')->fetchColumn();
        $finished = microtime(true);

        return ['slept' => $slept, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

// Each task reports its own start and finish rather than the batch being timed
// from here: this profile's async worker may still be finishing an earlier
// test's task when this one starts, which delays every task equally and says
// nothing about whether they overlapped.
$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));

// Keyed by promise ID, so it is re-indexed to keep the labels below positional.
foreach (array_values($results) as $i => $r) {
    $t->assertSame("task {$i} got the query result", (string) $r['slept'], '0');
    $t->assertGreaterThan(
        "task {$i} really waited for the server",
        $r['finished'] - $r['started'],
        0.9
    );
}

$t->assertTrue(
    'the three queries overlapped on one async worker (span < 2.0s, serial would be >= 3s)',
    $span < 2.0
);

$t->done();
