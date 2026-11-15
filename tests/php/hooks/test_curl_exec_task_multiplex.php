<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_exec_task_multiplex', 'hooks');

// The peer's HTTP port answers a request one second after the connection was
// accepted, and its HTTPS port does the same over TLS. curl_exec() waits for that
// answer inside libcurl, below anything PHP's stream layer can see, and a hooked
// one suspends the task fiber inside it. Two tasks on one async worker then wait
// together; without the suspension the thread serves one transfer after the
// other.
$scheme = ($_GET['scheme'] ?? 'http') === 'https' ? 'https' : 'http';
$port = $scheme === 'https' ? 8443 : 8080;
$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer, $scheme, $port, $i): array {
        $started = microtime(true);
        $ch = curl_init("{$scheme}://{$peer}:{$port}/1000/task{$i}");
        curl_setopt_array($ch, [
            CURLOPT_RETURNTRANSFER => true,
            CURLOPT_TIMEOUT => 6,
            CURLOPT_SSL_VERIFYPEER => false,
            CURLOPT_SSL_VERIFYHOST => 0,
        ]);
        $body = curl_exec($ch);
        $finished = microtime(true);

        return [
            'body' => $body === false ? 'error ' . curl_errno($ch) . ': ' . curl_error($ch) : $body,
            'started' => $started,
            'finished' => $finished,
        ];
    });
}
$results = array_values(oxphp_async_await_all($tasks));

foreach ($results as $i => $r) {
    $t->assertSame("{$scheme}: task {$i} got its own answer", $r['body'], "curl-done:/1000/task{$i}");
    $t->assertGreaterThan(
        "{$scheme}: task {$i} really waited for the answer",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    "{$scheme}: the two transfers overlapped on one async worker (span < 1.6s, serial would be >= 2s)",
    $span < 1.6
);

$t->done();
