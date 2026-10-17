<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool whose last reference goes away as its request ends is torn down by
// that request's own thread, but with no PHP code running there, so $destroy
// cannot be called. The slot is still freed, and the skip is counted under
// reason="no_frame" — once for a pool that has a $destroy, not at all for one
// that has none.
//
// This test holds one of the profile's two workers, so the requests it sends
// run one after the other on the other worker: one of them answering means the
// one before it has been torn down.
$t = new TestCase('test_pool_destroy_skipped_at_request_end_is_counted', 'pooltrad');

$get = static function (string $url): ?string {
    $ch = curl_init($url);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    return is_string($body) ? $body : null;
};
$fixture = static function (string $destroy) use ($get): ?array {
    $body = $get('http://127.0.0.1/tests/pooltrad/fixture_pool_left_idle.php?destroy=' . $destroy);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};
$skipped = static function () use ($get): ?array {
    $metrics = $get('http://127.0.0.1:9090/metrics');
    if ($metrics === null || !preg_match_all(
        '/^oxphp_shared_pool_destroy_skipped_total\{reason="(\w+)"\} (\d+)$/m',
        $metrics,
        $m,
        PREG_SET_ORDER,
    )) {
        return null;
    }
    $by = [];
    foreach ($m as [, $reason, $n]) {
        $by[$reason] = (int) $n;
    }
    ksort($by);
    return $by;
};

$before = $skipped();
$t->assertSame(
    'the skip counter is exported with all three reasons',
    array_keys($before ?? []),
    ['no_frame', 'other_thread', 'request_ended'],
);

$without = $fixture('0');
$with = $fixture('1');
$barrier = $fixture('0');
$t->assertTrue(
    'all three requests ran on the same worker, not this one',
    is_int($without['worker'] ?? null)
        && ($with['worker'] ?? null) === $without['worker']
        && ($barrier['worker'] ?? null) === $without['worker']
        && $without['worker'] !== oxphp_worker_id()
);
$t->assertSame('each left one slot idle', [$without['idle'] ?? null, $with['idle'] ?? null], [1, 1]);

$after = $skipped();
$t->assertSame(
    'one slot went without its $destroy, the one of the pool that had one',
    $after,
    isset($before['no_frame']) ? ['no_frame' => $before['no_frame'] + 1] + $before : null,
);

$t->done();
