<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_race_one_connection', 'hooksdb');

// Several requests building the same persistent connection at the same moment.
// This is what a fresh worker meets, not an edge case: an application whose
// handler begins `static $pdo ??= new PDO(...)` reaches that line from every
// request that arrives before the first one has finished connecting, and a
// connect is exactly the kind of wait that lets the others run.
//
// PDO looks a pooled connection up before it connects and registers the result
// afterwards, so overlapping constructors all miss the pool and the last
// registration replaces the earlier ones. Replacing an entry frees the connection
// behind it — while the request that built it is still reading its own reply on
// it, which is where the fatals and the crash came from.
//
// Its own key per run, so the race starts from an empty pool every time rather
// than only on the first run against a fresh worker.
$sharedState['ctor_race_key'] = 'ctor-race-' . bin2hex(random_bytes(4));

$request = function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_race.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

$tasks = [];
for ($i = 0; $i < 4; $i++) {
    $tasks[] = oxphp_async($request);
}

$ids = [];
$spans = [];
$failed = [];
foreach ($tasks as $task) {
    $body = oxphp_async_await($task)['body'];
    if (preg_match('/^ctor-race-done:(\d+) (\d+\.\d+) (\d+\.\d+)$/m', $body, $m) === 1) {
        $ids[] = $m[1];
        $spans[] = [(float) $m[2], (float) $m[3]];
        continue;
    }
    foreach (explode("\n", $body) as $line) {
        if (str_starts_with($line, 'ctor-race-') || str_starts_with($line, 'HTTP/')) {
            $failed[] = trim($line);
        }
    }
}

$t->assertSame(
    'every racing request finished its own query: ' . implode(' | ', $failed),
    count($ids),
    4
);

// The mechanism, not the symptom. Four requests naming one persistent connection
// must end up on one connection: a build where the constructors overlap without
// seeing each other hands each of them a connection of its own, and each new
// registration drops the one before it — which is the same event as the fatal,
// seen from the side that causes it rather than the side that is hit by it.
$t->assertCount(
    'and they all did it on the one persistent connection they asked for, not on '
        . count(array_unique($ids)) . ' separate ones',
    array_unique($ids),
    1
);

// The premise the assertion above rests on, checked rather than assumed: at least
// two of these constructors were inside PDO at the same moment. A worker that
// served the four one after another would have each of them find the connection
// its predecessor registered and report the same id truthfully, exercising none of
// this — and the assertion above would pass while proving nothing. The spans cover
// the constructor alone, which is where the race is, and they overlap on a fixed
// build too: waiting for the constructor ahead happens inside the call.
$overlapped = false;
foreach ($spans as $i => $a) {
    foreach ($spans as $j => $b) {
        if ($i !== $j && $a[0] < $b[1] && $b[0] < $a[1]) {
            $overlapped = true;
        }
    }
}
$t->assertTrue(
    'at least two of the four were building their connection at the same moment,'
        . ' so this run raced rather than queued',
    $overlapped
);

// The worker kept serving. A connection dropped under a parked reader ends that
// request; freeing it under the object still holding it ends the process.
$after = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_db_sleep.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$t->assertContains('the worker served the next request as usual', oxphp_async_await($after)['body'], 'db-done');

$t->done();
