<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_multi_select_task_multiplex', 'hooks');

// The loop every curl_multi client runs — curl_multi_exec() to move the transfers
// on, curl_multi_select() to wait for the next thing to happen — with the answer a
// second away. The wait is libcurl's, inside curl_multi_select(), and a hooked one
// suspends the task fiber there. Two tasks on one async worker then wait together.
$peer = net_peer_ip();

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($peer, $i): array {
        $started = microtime(true);
        $mh = curl_multi_init();
        $ch = curl_init("http://{$peer}:8080/1000/multi{$i}");
        curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 6]);
        curl_multi_add_handle($mh, $ch);

        $codes = [];
        $deadline = $started + 6.0;
        do {
            $codes[] = curl_multi_exec($mh, $running);
            if ($running > 0) {
                curl_multi_select($mh, 2.0);
            }
        } while ($running > 0 && microtime(true) < $deadline);
        $finished = microtime(true);

        $body = curl_multi_getcontent($ch);
        curl_multi_remove_handle($mh, $ch);

        return [
            'body' => $body,
            'codes' => array_values(array_unique($codes)),
            'started' => $started,
            'finished' => $finished,
        ];
    });
}
$results = array_values(oxphp_async_await_all($tasks));

foreach ($results as $i => $r) {
    $t->assertSame("task {$i} got its own answer", $r['body'], "curl-done:/1000/multi{$i}");
    $t->assertSame("task {$i}: every curl_multi_exec() succeeded", $r['codes'], [CURLM_OK]);
    $t->assertGreaterThan(
        "task {$i} really waited for the answer",
        $r['finished'] - $r['started'],
        0.9
    );
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertTrue(
    'the two curl_multi_select() waits overlapped on one async worker (span < 1.6s, serial would be >= 2s)',
    $span < 1.6
);

$t->done();
