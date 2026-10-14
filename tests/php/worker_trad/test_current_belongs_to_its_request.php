<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Outside worker mode every request is a PHP request of its own, and PHP frees
// the objects a request created when it ends. Worker::current() must therefore
// hand each request an instance created in that request — the same one for
// every call within it — and never the one an earlier request on the thread
// got.
//
// This test holds one of the profile's two workers, so the three requests it
// sends run one after the other on the other worker: the first one there may
// create the instance, and the next two are the requests that would be handed
// a previous request's. Each creates a different number of objects before its
// call — see fixture_current.php.
$t = new TestCase('test_current_belongs_to_its_request', 'worker_trad');

$fixture = static function (int $pad): ?array {
    $ch = curl_init('http://127.0.0.1/tests/worker_trad/fixture_current.php?pad=' . $pad);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$runs = [$fixture(0), $fixture(1), $fixture(2)];

foreach ($runs as $i => $run) {
    $n = $i + 1;
    $t->assertNotNull("request {$n} answered", $run);
    $t->assertTrue(
        "request {$n}: the instance was created by this request, right after its second object",
        $run['created_here'] ?? null
    );
    $t->assertTrue("request {$n}: no two live objects share a handle", $run['distinct'] ?? null);
    $t->assertTrue("request {$n}: the instance is still a Worker once another object is created", $run['is_worker'] ?? null);
    $t->assertTrue("request {$n}: a second call returns the same instance", $run['same_within_request'] ?? null);
    $t->meta("ids_{$n}", $run['ids'] ?? null);
}

$workers = array_map(static fn (?array $run) => $run['worker'] ?? null, $runs);
$t->assertTrue(
    'all three requests ran on the same worker, not this one',
    is_int($workers[0])
        && $workers[0] !== oxphp_worker_id()
        && count(array_unique($workers, SORT_REGULAR)) === 1
);

$t->done();
