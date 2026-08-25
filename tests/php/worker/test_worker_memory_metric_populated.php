<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Behavioural contract: the per-worker gauge and the per-worker request
// counter are written together at the end of every worker-mode request, so a
// worker that has served one has both. Asserting the pair rather than "the
// gauge is non-zero" is what pins the mechanism: a worker slot carrying a
// request count but no memory can only mean the fill for one of the two is
// not connected — which is exactly how this metric spent its whole life
// exporting a flat zero while the counter next to it moved.

$test = new TestCase('worker_memory_metric_populated', 'worker');

$metrics = @file_get_contents('http://127.0.0.1:9090/metrics');
$test->assertTrue('metrics endpoint reachable', is_string($metrics));

if (!is_string($metrics)) {
    $test->done();
}

/**
 * @return array<string, float> worker label => value
 */
$series = static function (string $metrics, string $name): array {
    preg_match_all(
        '/^' . preg_quote($name, '/') . '\{worker="(\d+)"\} ([\d.e+-]+)$/mi',
        $metrics,
        $matches,
        PREG_SET_ORDER
    );
    $out = [];
    foreach ($matches as $m) {
        $out[$m[1]] = (float)$m[2];
    }
    return $out;
};

$memory = $series($metrics, 'oxphp_worker_memory_bytes');
$requests = $series($metrics, 'oxphp_worker_requests_count');

$test->assertNotEmpty('oxphp_worker_memory_bytes is exposed', $memory);

// Workers that have completed at least one request — the only ones whose
// gauge has had a chance to be written.
$busy = array_keys(array_filter($requests, static fn(float $n): bool => $n > 0));
$test->assertNotEmpty('at least one worker has completed a request', $busy);

$zeroed = [];
foreach ($busy as $worker) {
    if (($memory[$worker] ?? 0.0) <= 0.0) {
        $zeroed[] = $worker;
    }
}
$test->assertEmpty(
    'every worker with a request count has a memory value (zero for workers: '
        . (implode(',', $zeroed) ?: '-') . ')',
    $zeroed
);

// The gauge is the PHP heap of one worker thread, not the resident set of the
// whole process: it has to come in strictly under RSS. Skips itself where RSS
// is unavailable (non-Linux, restrictive kernel), where the accessor returns 0.
$rss = OxPHP\Server\Worker::current()->rss();
$peak = $memory === [] ? 0.0 : max($memory);
if ($rss > 0) {
    $test->assertTrue(
        "gauge ($peak) is below process RSS ($rss)",
        $peak > 0.0 && $peak < (float)$rss
    );
}

$test->meta('memory', $memory);
$test->meta('requests', $requests);
$test->meta('rss', $rss);

$test->done();
