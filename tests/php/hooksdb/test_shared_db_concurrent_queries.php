<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_db_concurrent_queries', 'hooksdb');

// The other shape of the shared-connection case, and the one the "long holder,
// late prober" tests cannot reach: several complete short exchanges arriving on
// top of each other. Each request here runs one sub-millisecond query on the
// connection the worker shares, so every fiber both takes and releases the
// connection inside the window the others are in — which is where a claim that is
// taken, or released, or handed over at the wrong moment shows up and a claim that
// merely holds a long exchange open does not.
//
// The connection is created here first, deliberately: leaving four concurrent
// requests to race on `??=` would test the fixture's initialisation rather than
// the server, and the losers of that race drop connections other fibers are on.
//
// Created and not used: a claim is held until the request that took it ends, so a
// query here would keep the connection for as long as this request waits for the
// four below — which need it — and the two would come apart only on the claim's
// own deadline. The constructor connects, which is all this needs.
//
// Its own key, not the one the other shared-connection tests use: those assert
// that their holder request created the connection, and a test that created it
// earlier would leave that premise true without a holder behind it. Suite order
// then decides whether they still test anything, which is not a property to
// leave resting on the order of lines in a file.
$sharedState['pdo_concurrent'] ??= new PDO(
    'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
    getenv('DB_USER') ?: 'appuser',
    getenv('DB_PASS') ?: 'apppass',
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);

$request = function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_db_load.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

$tasks = [];
for ($i = 0; $i < 4; $i++) {
    $tasks[] = oxphp_async($request);
}

$bodies = [];
foreach ($tasks as $task) {
    $bodies[] = oxphp_async_await($task)['body'];
}

$ok = 0;
$failed = [];
$spans = [];
foreach ($bodies as $body) {
    if (preg_match('/^load-ok (\d+\.\d+) (\d+\.\d+)$/m', $body, $m) === 1) {
        $ok++;
        $spans[] = [(float) $m[1], (float) $m[2]];
        continue;
    }
    // Keep only the outcome line; the rest of the body is HTTP headers.
    foreach (explode("\n", $body) as $line) {
        if (str_starts_with($line, 'load-')) {
            $failed[] = trim($line);
        }
    }
}

$t->assertSame(
    'every concurrent request got the answer to its own query'
        . ($failed === [] ? '' : ' (' . implode(' | ', $failed) . ')'),
    $ok,
    4
);

// What this rules out, and only this: that the worker served the four one after
// another, which would make the assertion above say nothing about sharing. It is
// not evidence of queries running in parallel — the claim exists precisely to stop
// that, so the span of a request that waited its turn overlaps the span of the one
// it waited for, and that is the expected shape here.
$overlapped = false;
foreach ($spans as $i => $a) {
    foreach ($spans as $j => $b) {
        if ($i !== $j && $a[0] < $b[1] && $b[0] < $a[1]) {
            $overlapped = true;
        }
    }
}
$t->assertTrue(
    'the worker had at least two of these requests in flight at once, rather than serving'
        . ' them one after another',
    $overlapped
);

$t->done();
