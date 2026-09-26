<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_gate_per_source', 'hooksdb');

// Persistent PDO constructors on one worker wait for each other, because two of
// them overlapping on the same pool key both miss the pool and the second
// registration frees the connection the first is using. That wait is only needed
// between constructors that can land on the same pool entry — and a constructor
// to a server that accepts the connection and never answers stays inside PDO for
// as long as mysqlnd's read timeout allows, a day out of the box. Were every
// persistent constructor on the worker queued behind it, each one to a healthy
// database would spend its whole wait budget there and then go ahead without
// waiting at all, which is the race the wait exists to prevent.
//
// Which constructors share a wait is decided from a hash of their data source, so
// the two DSNs here are chosen to be as close as two DSNs to different servers can
// be: the same text but for two neighbouring characters, `host=` against `hou2=`.
// The driver ignores a key it does not know and lets a later one win, so the first
// reaches the healthy database and the second the black hole on this worker's own
// 127.0.0.1:3306. For a string hash that multiplies by 33 per character (the
// engine's own) the pair is an exact collision; nothing short of the text itself
// should tell the two apart.
$db = getenv('DB_NAME') ?: 'appdb';
$dbHost = getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql';
$server = @stream_socket_server('tcp://127.0.0.1:3306', $errno, $errstr);
if ($server === false) {
    $t->assertTrue("the black-hole listener bound: {$errstr} ({$errno})", false);
    $t->done();
    return;
}

$sharedState['black_hole_dsn'] = "mysql:host=127.0.0.1;port=3306;dbname={$db};hou2={$dbHost}";
$sharedState['ctor_race_key'] = 'ctor-gate-' . bin2hex(random_bytes(4));
unset($sharedState['black_hole_entered'], $sharedState['black_hole_left']);

// One more constructor to the healthy database, named through a `uri:` file. The
// server cannot tell which data source such a DSN names without reading it again,
// so that constructor waits until no constructor to any source is running — which,
// behind the black hole, is never. What matters here is that it waits holding
// nothing: were it to hold the others up while it waits, the healthy constructors
// would be stuck behind the black hole through it.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$uriFile = '/tmp/oxphp-ctor-dsn-' . bin2hex(random_bytes(4));
file_put_contents($uriFile, $dsn);
$sharedState['ctor_race_uri_file'] = $uriFile;
$sharedState['ctor_race_vias'] = ['uri', 'collide', 'collide'];

$request = function (string $path, int $timeout): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, $timeout);
    fwrite($sock, "GET /tests/hooksdb/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

$holder = oxphp_async($request, 'fixture_persistent_ctor_black_hole.php', 20);

// Until the holder is inside its constructor, and then long enough for it to be
// parked on the greeting it will never get rather than still connecting.
for ($i = 0; $i < 100 && !isset($sharedState['black_hole_entered']); $i++) {
    oxphp_usleep(20_000);
}
oxphp_usleep(200_000);

$t->assertTrue(
    'the black-hole constructor started before the healthy ones',
    isset($sharedState['black_hole_entered'])
);

$viaUri = oxphp_async($request, 'fixture_persistent_ctor_race.php', 15);
oxphp_usleep(100_000);

$healthy = [];
for ($i = 0; $i < 2; $i++) {
    $healthy[] = oxphp_async($request, 'fixture_persistent_ctor_race.php', 15);
}

$ids = [];
$builds = [];
$spans = [];
$failed = [];
foreach ($healthy as $task) {
    $body = oxphp_async_await($task)['body'];
    if (preg_match('/^ctor-race-via:collide$/m', $body) === 1
        && preg_match('/^ctor-race-done:(\d+) (\d+\.\d+) (\d+\.\d+)$/m', $body, $m) === 1) {
        $ids[] = $m[1];
        $builds[] = (float) $m[3] - (float) $m[2];
        $spans[] = [(float) $m[2], (float) $m[3]];
        continue;
    }
    foreach (explode("\n", $body) as $line) {
        if (str_starts_with($line, 'ctor-race-') || str_starts_with($line, 'HTTP/')) {
            $failed[] = trim($line);
        }
    }
}

// The premise, read at the moment it matters rather than latched: the black-hole
// constructor had not left PDO when both healthy ones had finished. Without this
// the timing below would also pass on a run where the holder failed fast and
// there was nothing to wait behind.
$holderStillInside = isset($sharedState['black_hole_entered']) && !isset($sharedState['black_hole_left']);

$t->assertSame(
    'both healthy requests finished their own query: ' . implode(' | ', $failed),
    count($ids),
    2
);

$t->assertTrue(
    'the black-hole constructor was still waiting for its greeting when both healthy ones had finished',
    $holderStillInside
);

// The mechanism. A healthy constructor does not wait behind one to another data
// source: its build takes a connect, not the wait budget (two seconds in this
// image). A build that queued behind the black hole comes out at that budget, and
// then went ahead unserialised.
$slowest = $builds === [] ? INF : max($builds);
$t->assertLessThan(
    sprintf('no healthy constructor waited behind the black-hole one (slowest build %.3fs)', $slowest),
    $slowest,
    1.0
);

// And the wait that is still needed still happens: the two healthy constructors
// share one pool key, overlapped, and ended up on one connection.
$overlapped = count($spans) === 2
    && $spans[0][0] < $spans[1][1] && $spans[1][0] < $spans[0][1];
$t->assertTrue('the two healthy constructors were inside PDO at the same moment', $overlapped);
$t->assertCount(
    'and they share the one persistent connection they asked for, not '
        . count(array_unique($ids)) . ' separate ones',
    array_unique($ids),
    1
);

// The `uri:` constructor, for its part, was still waiting when the healthy ones
// were done — the premise of the paragraph above — and gave up at the budget
// rather than never. What it did then is the limit of this protection, not a
// guarantee: it went ahead without waiting. Here that races nothing, because its
// file names the database without the extra `host=` the healthy ones carry, so
// its pool key is a different one and it opens a connection of its own.
$uriBody = oxphp_async_await($viaUri)['body'];
$u = [];
$uriBuild = preg_match('/^ctor-race-via:uri\nctor-race-done:\d+ (\d+\.\d+) (\d+\.\d+)$/m', $uriBody, $u) === 1
    ? (float) $u[2] - (float) $u[1]
    : 0.0;
$t->assertGreaterThan(
    sprintf('the uri: constructor waited behind the black hole until its budget ran out (%.3fs)', $uriBuild),
    $uriBuild,
    1.5
);
$t->assertTrue(
    'and its wait outlasted the healthy constructors, which did not wait behind it',
    $spans !== [] && isset($u[2]) && (float) $u[2] > max(array_column($spans, 1))
);
unset($sharedState['ctor_race_vias'], $sharedState['ctor_race_uri_file']);
@unlink($uriFile);

// Closing the listener resets the connection the holder is parked on, so it
// leaves its constructor with an error instead of after a day.
fclose($server);
$held = oxphp_async_await($holder)['body'];
$t->assertContains('the black-hole constructor ended in a connection error', $held, 'black-hole-failed:');

$after = oxphp_async($request, 'fixture_db_sleep.php', 10);
$t->assertContains('the worker served the next request as usual', oxphp_async_await($after)['body'], 'db-done');

$t->done();
