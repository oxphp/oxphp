<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Behavioural contract: while a request is in flight, the supervisor
// scans (1s period) write the in-flight age into
// oxphp_worker_request_age_seconds for the busy worker. Sleeping past
// at least one scan and then scraping /metrics from the same request
// proves the gauge is populated end-to-end (heartbeat write →
// supervisor scan → Prometheus exposition).

$test = new TestCase('request_age_metric', 'observability');

// 2 s gives the supervisor at least one scan after the heartbeat
// write at request setup, regardless of where in its 1 s cycle the
// scan lands when this request begins.
sleep(2);

$metrics = @file_get_contents('http://127.0.0.1:9090/metrics');
$test->assertTrue('metrics endpoint reachable', is_string($metrics));

if (!is_string($metrics)) {
    $test->done();
    return;
}

preg_match_all(
    '/^oxphp_worker_request_age_seconds\{worker_id="\d+"\} ([\d.]+)$/m',
    $metrics,
    $matches
);
$values = array_map('floatval', $matches[1] ?? []);
$max_age = $values === [] ? 0.0 : max($values);

$test->assertTrue(
    'at least one worker_request_age_seconds line is exposed',
    $values !== []
);
$test->assertTrue(
    'busy worker age > 1.0s (was ' . $max_age . ')',
    $max_age > 1.0
);

$test->done();
