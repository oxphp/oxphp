<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_off_holds_thread', 'hookscurloff');

// The control for test_curl_exec_task_multiplex. This profile runs with
// RUNTIME_HOOKS=sleep,streams,net — every category but curl — and the same two
// transfers on one async worker must now run one after the other: the wait is
// inside libcurl, which only the curl category parks. The net category stays on
// beside it, and test_net_tls_read_task_multiplex in this profile still has to
// overlap, so the categories are switched separately.
$config = json_decode((string) @file_get_contents('http://127.0.0.1:9090/config'), true);
$t->assertSame(
    'the process published the categories it was given, without curl',
    is_array($config) ? ($config['runtime_hooks'] ?? null) : null,
    ['sleep', 'streams', 'net']
);

$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer, $i): array {
        $started = microtime(true);
        $ch = curl_init("http://{$peer}:8080/1000/task{$i}");
        curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 6]);
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
    $t->assertSame("task {$i} still got its answer", $r['body'], "curl-done:/1000/task{$i}");
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertGreaterThan(
    'the two transfers ran one after the other: each held the async worker thread for its whole wait',
    $span,
    1.8
);

$t->done();
