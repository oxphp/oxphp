<?php

declare(strict_types=1);

// A fatal in one request leaves the objects other requests on the worker hold,
// and those the worker itself holds, their destructors; the failed request's
// own objects are let go of without theirs, as PHP lets go of them after a
// fatal anywhere. One of its objects that outlives that, here in a reference
// cycle, has its destructor run when the cycle collector frees it.
//
// This request holds an object of its own and one in a static, then parks on
// the read of its inner request's answer. The inner request holds an object and
// fails while this one waits. On its way into that bailout the engine flags
// every object alive on the thread as already destructed, its neighbours' too,
// and an object so flagged is freed without its destructor. ?oom=1 fails the
// inner request by running out of memory, which the engine reports from a
// later point and with the same flag; ?timeout=1 by running out of time.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/destruct_probe.php';

$variant = isset($_GET['oom']) ? 'oom' : (isset($_GET['timeout']) ? 'timeout' : null);
$t = new TestCase('fatal_keeps_a_parked_requests_destructors' . ($variant !== null ? '_' . $variant : ''), 'fibers');

OxphpDestructProbe::reset(['parked', 'kept', 'failed', 'failed_cycle']);

$parked = new OxphpDestructProbe('parked');
OxphpDestructProbe::$kept = new OxphpDestructProbe('kept');

$body = fiber_inner_request('/tests/fibers/fixture_fatal_beside_a_parked_request.php' . ($variant !== null ? '?' . $variant . '=1' : ''), 10.0);
$t->assertNotContains('the inner request failed before its end', $body, 'not reached');

unset($parked);
OxphpDestructProbe::$kept = null;
gc_collect_cycles();

$t->assertTrue("the destructor of this request's object ran", OxphpDestructProbe::destructed('parked'));
$t->assertTrue("the destructor of the worker's object ran", OxphpDestructProbe::destructed('kept'));
$t->assertFalse("the failed request's object had no destructor run", OxphpDestructProbe::destructed('failed'));
$t->assertTrue("the destructor of the failed request's object in a cycle ran once the collector freed it", OxphpDestructProbe::destructed('failed_cycle'));

$t->done();
