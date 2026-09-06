<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_ping_under_concurrent_use', 'hooksdb');

// PDO's liveness check on a pooled connection is the one command on that
// connection that no claimed call stands in front of: PDO calls it from inside the
// constructor, straight through the driver's own method table. The answer given to
// a constructor whose connection another fiber holds keeps it off the wire in that
// case — but a pooled connection is not held for most of its life. A claim runs
// from a request's first query to the end of that request, so between two requests
// the connection is live, referenced by the handle the worker keeps, and claimed by
// nobody. That is when the check reaches the wire.
//
// What that costs is not a stale answer. The ping parks the constructor's fiber
// inside a socket read, and while it is parked another request's query finds the
// connection unclaimed, takes it, and writes into the exchange the ping is halfway
// through. The socket refuses that write, and mysqlnd does not treat a refused
// write as a failed call: it marks the connection gone and closes it, after which
// every command on it is answered "server has gone away" from that state alone,
// for as long as the worker keeps the handle — which, for an application holding
// one connection per worker, is the rest of its life.
//
// So the scenario is a burst of both kinds of request on one pooled connection:
// requests that construct a handle off the pool key, and requests that only use
// the one the worker already has.
$sharedState['ping_key'] = 'ctor-ping-' . bin2hex(random_bytes(4));
$sharedState['ping_handles'] = [];
unset($sharedState['ping_pdo']);

// The task body is built per fixture rather than captured from here, so what each
// task closes over is a string and nothing else.
$fetch = static function (string $fixture): \Closure {
    return static function () use ($fixture): string {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return "connect failed: {$errstr} ({$errno})";
        }
        stream_set_timeout($sock, 15);
        fwrite($sock, "GET /tests/hooksdb/{$fixture} HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
        $body = (string) stream_get_contents($sock);
        fclose($sock);

        return $body;
    };
};

// ── The connection under test ─────────────────────────────────────────────────
// Created by a request of its own, and by one that finishes: this request's own
// claim would otherwise stand over the connection for as long as the burst below
// runs, which is the one state in which the check never reaches the wire — the
// test would then pass without exercising anything.
$createBody = oxphp_async_await(oxphp_async($fetch('fixture_persistent_ping_create.php')));
preg_match('/^ping-create-done: id:(\d+)$/m', $createBody, $m);
$createdId = $m[1] ?? '';
$t->assertMatch(
    'a first request pooled the connection and finished, leaving it live and unclaimed: '
        . str_replace("\n", ' ', $createBody),
    $createdId,
    '/^\d+$/'
);

// ── The burst ─────────────────────────────────────────────────────────────────
$tasks = [];
for ($i = 0; $i < 6; $i++) {
    $tasks[] = oxphp_async($fetch('fixture_persistent_ping_ctor.php'));
    $tasks[] = oxphp_async($fetch('fixture_persistent_ping_query.php'));
}

$bodies = [];
foreach ($tasks as $task) {
    $bodies[] = oxphp_async_await($task);
}

$ids = [];
$failed = [];
$spans = ['ctor' => [], 'query' => []];
foreach ($bodies as $body) {
    if (preg_match('/^ping-(ctor|query)-done: id:(\d+) (\d+\.\d+) (\d+\.\d+)$/m', $body, $m) === 1) {
        $ids[] = $m[2];
        $spans[$m[1]][] = [(float) $m[3], (float) $m[4]];
        continue;
    }
    // Keep only the outcome line; the rest of the body is HTTP headers.
    $outcome = '';
    foreach (explode("\n", $body) as $line) {
        if (str_starts_with($line, 'ping-')) {
            $outcome = trim($line);
        }
    }
    // A response carrying no outcome line at all did not reach the fixture's own
    // reporting — a server error, or a request that never arrived. Its status line
    // is the only thing that says which, and without it the count below would be
    // short with nothing to explain why.
    $failed[] = $outcome !== '' ? $outcome : 'no outcome line, status: ' . trim(strtok($body, "\n"));
}

$t->assertSame(
    'every request in the burst got the answer to its own query'
        . ($failed === [] ? '' : ' (' . implode(' | ', $failed) . ')'),
    count($ids),
    12
);

// The premise the rest of this rests on: the constructors adopted the pooled
// connection rather than opening one each. Without it a green run would say
// nothing — a constructor that got a connection of its own never asks the
// question this test is about.
$t->assertSame(
    'and every one of them was on the connection the first request pooled',
    array_values(array_unique($ids)),
    [$createdId]
);

// The second premise, and the one a serialised run would fail: the two kinds of
// request were on the worker at the same time. Requests served one after another
// cannot land inside each other's exchange, so a green run without this says only
// that nothing overlapped.
$overlapped = false;
foreach ($spans['ctor'] as $a) {
    foreach ($spans['query'] as $b) {
        if ($a[0] < $b[1] && $b[0] < $a[1]) {
            $overlapped = true;
        }
    }
}
$t->assertTrue(
    'and the worker had a constructing request and a querying one in flight at once,'
        . ' rather than serving them one after another',
    $overlapped
);

// Named separately from the count above, because this is the shape the defect
// has and a count says only that something went wrong.
$t->assertNotContains(
    'and none of them was answered from a connection mysqlnd had marked gone',
    implode(' | ', $failed),
    'server has gone away'
);

// ── After the burst ───────────────────────────────────────────────────────────
// The other half of the guarantee. A connection mysqlnd has marked gone never
// comes back: it answers every later command from that state without touching the
// wire, and the worker holding the handle never builds another. So a burst that
// ended without an error is not yet evidence the connection survived it — this is.
$afterId = '';
$afterError = '';
try {
    $afterId = (string) $sharedState['ping_pdo']->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $afterError = str_replace("\n", ' ', $e->getMessage());
}

$t->assertSame(
    'the connection the worker keeps still answers after the burst: ' . $afterError,
    $afterError,
    ''
);
$t->assertSame('and it is the one it had all along', $afterId, $createdId);

$t->done();
