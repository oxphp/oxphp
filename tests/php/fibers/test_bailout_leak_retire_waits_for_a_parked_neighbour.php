<?php

declare(strict_types=1);

// A worker that is to be retired for what a request left behind is not retired
// while another request is parked on it.
//
// This request sends the neighbour fixture a request of its own and parks on the
// read of its answer, so it is the parked request: the neighbour is ended inside
// usort() while this one waits, and the retire it earns must come only once there
// is nobody left to lose. A retire that came at the neighbour's end would take
// this request with it, and the suite line expects it to be answered 200. The
// probe on the next line reads that the worker was retired all the same, after.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$t = new TestCase('bailout_leak_retire_waits_for_a_parked_neighbour', 'fibers');

OxphpBailoutLeak::arm('neighbour');

$body = fiber_inner_request('/tests/fibers/fixture_bailout_in_usort.php', 10.0);

$t->assertNotContains('the neighbour was ended by its time limit, not run to its end', $body, 'finished');

$t->done();
