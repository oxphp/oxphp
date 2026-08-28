<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Behavioural contract: the census is written by the worker's serve loop, and
// nothing between the pointer it writes through and the gauge that renders it
// is exercised by a unit test — the install sits behind the php feature and the
// writes are in C. A census that is never filled renders as a flat zero on
// every worker, which is indistinguishable from an idle pool and is precisely
// how the memory gauge next door spent its whole life exporting nothing.
//
// This request is being served by a worker as it reads, so that worker is
// carrying at least this one fiber. Asserting against the worker's own id, not
// against "some worker is non-zero", is what pins the fill to the thread that
// does it.

$test = new TestCase('worker_fiber_census_populated', 'worker');

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

$census = $series($metrics, 'oxphp_worker_request_fibers_active');

$test->assertNotEmpty('oxphp_worker_request_fibers_active is exposed', $census);

if ($census === []) {
    $test->done();
}

// The label is the stats slot, which a worker takes by its id modulo the number
// of slots — the same arithmetic the server does when handing a worker thread
// its slot, so a recycled worker with an id past the pool size still resolves.
$id = OxPHP\Server\Worker::current()->id();
$label = (string)($id % count($census));

$test->assertKeyExists('the worker serving this request has a census entry', $census, $label);

$test->assertGreaterThan(
    "the worker serving this request counts it (worker $label of "
        . json_encode($census) . ')',
    $census[$label] ?? 0.0,
    0.0
);

$test->meta('census', $census);
$test->meta('worker', $id);

$test->done();
