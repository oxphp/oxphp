<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_connect_made_in_warning_handler_keeps_outer_connect_parked', 'hooks');

// The engine tries a connect's addresses one after another, and a bindto it cannot
// bind is reported as a warning in the middle of that loop — after which the
// connect carries on to its wait. An error handler runs there, inside the connect,
// and one that connects somewhere itself (to write its log to a socket, say) starts
// a second connect while the first is still going on. When the second ends, the
// first is still a connect, and its wait must still suspend the fiber; a build that
// takes the end of the inner connect for the end of the outer one holds the thread
// for it.
//
// ASYNC_WORKERS=1: two tasks each connect to an address that answers nothing, with
// a one-second timeout and a bindto that cannot be bound, and an error handler that
// makes a connect of its own to the peer, which accepts it and says nothing. The
// two outer waits have to run side by side on the one async worker thread. Each
// task reports when its connect started and finished rather than timing the batch
// from here, for the reason the plain connect multiplex test does.
$silent = net_silent_address(7);
$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function (string $silent, string $peer): array {
        $handled = 0;
        $inner_connected = false;
        set_error_handler(static function () use (&$handled, &$inner_connected, $peer): bool {
            $handled++;
            $inner = stream_socket_client("tcp://{$peer}:4436", $errno, $errstr, 3.0);
            if (is_resource($inner)) {
                $inner_connected = true;
                fclose($inner);
            }
            return true;
        });
        // 192.0.2.1 is reserved for documentation: no interface holds it, so the
        // bind fails, with a warning, and the connect goes on without it.
        $context = stream_context_create(['socket' => ['bindto' => '192.0.2.1:0']]);

        $started = microtime(true);
        $sock = stream_socket_client(
            "tcp://{$silent}:80",
            $errno,
            $errstr,
            1.0,
            STREAM_CLIENT_CONNECT,
            $context
        );
        $finished = microtime(true);
        restore_error_handler();
        if (is_resource($sock)) {
            fclose($sock);
        }
        return [
            'connected' => $sock !== false,
            'handled' => $handled,
            'inner_connected' => $inner_connected,
            'started' => $started,
            'finished' => $finished,
        ];
    }, $silent, $peer);
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertTrue("task {$i}: the error handler ran for the bindto warning", $r['handled'] >= 1);
    $t->assertTrue("task {$i}: the connect the handler made went through", $r['inner_connected']);
    $t->assertFalse("task {$i}: nothing answers at that address, so the outer connect did not succeed", $r['connected']);
    $t->assertGreaterThan(
        "task {$i}: the outer connect really waited out its timeout",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    'the two outer connects overlapped on one async worker (span < 1.6s, serial would be >= 2s)',
    $span < 1.6
);

$t->done();
