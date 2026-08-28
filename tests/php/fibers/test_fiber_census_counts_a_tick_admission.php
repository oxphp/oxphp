<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';

// A worker publishes how many request fibers it is carrying once per turn of
// its serve loop, and a turn spans a whole tick of the event loop. The tick
// admits requests of its own and runs them inline, so a request admitted there
// executes entirely between two publications — and unless the tick says so
// itself, the number standing while it runs is the one from before it arrived.
//
// That is not an off-by-one at the margin. The loop enters its event-loop
// branch whenever a deferred promise drain is outstanding, including with no
// fibers at all, so the reading left standing over such a request is zero: a
// worker reporting an idle scheduler while executing a request, which is the
// one thing this gauge exists to rule out.
//
// The outer request parks on the read of the inner one, so the worker carries
// two fibers — this one and the request the tick admitted — and the inner
// request reports what the worker says about itself from inside that window.

$body = fiber_inner_request('/tests/fibers/fixture_fiber_census_probe.php');
$data = json_decode($body, true);

$t = new TestCase('fiber_census_counts_a_tick_admission', 'fibers');

$t->assertTrue('inner request answered with JSON', is_array($data));

if (!is_array($data)) {
    $t->meta('body', $body);
    $t->done();
}

$t->assertTrue('inner request scraped the metrics endpoint', ($data['scraped'] ?? false) === true);
$t->assertNotEmpty('oxphp_worker_request_fibers_active is exposed', $data['series'] ?? []);

$t->assertSame(
    'the worker counted both fibers it was carrying: the parked request and the one the tick admitted'
        . ' (series: ' . json_encode($data['series'] ?? []) . ')',
    // Cast: the census crosses a JSON hop between the two requests, and an
    // integral float lands on this side as an int. The count is what is being
    // asserted, not its PHP type.
    is_numeric($data['own'] ?? null) ? (float) $data['own'] : null,
    2.0
);

$t->meta('census', $data);

$t->done();
