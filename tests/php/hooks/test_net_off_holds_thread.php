<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_off_holds_thread', 'hooksnetoff');

// The control for test_net_tls_read_task_multiplex. This profile runs with
// RUNTIME_HOOKS=sleep,streams — the hooks of the other profiles, without the net
// category — and the same two TLS reads on one async worker must now serialize:
// the wait is inside the engine's TLS read, which only the net category parks.
//
// Without this, the multiplex tests prove the category does something but not that
// it is the category doing it: a build that parked TLS waits whatever the setting
// would pass all of them.
$config = json_decode((string) @file_get_contents('http://127.0.0.1:9090/config'), true);
$t->assertSame(
    'the process published the categories it was given, without net',
    is_array($config) ? ($config['runtime_hooks'] ?? null) : null,
    ['sleep', 'streams']
);

$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer): array {
        $context = stream_context_create(['ssl' => ['verify_peer' => false, 'verify_peer_name' => false]]);
        $started = microtime(true);
        $sock = @stream_socket_client("ssl://{$peer}:4433", $errno, $errstr, 3.0, STREAM_CLIENT_CONNECT, $context);
        if ($sock === false) {
            return ['body' => 'connect failed', 'started' => $started, 'finished' => microtime(true)];
        }
        stream_set_timeout($sock, 6);
        $body = (string) fread($sock, 16);
        $finished = microtime(true);
        fclose($sock);

        return ['body' => $body, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertSame("task {$i} still read the peer's answer", $r['body'], 'tls-done');
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertGreaterThan(
    'the two reads ran one after the other: each held the async worker thread for its whole wait',
    $span,
    1.8
);

$t->done();
