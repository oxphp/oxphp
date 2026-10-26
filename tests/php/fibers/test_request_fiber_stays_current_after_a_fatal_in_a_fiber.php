<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';

// A request that fatals inside a Fiber it started leaves the fiber it ran in
// able to say so: Fiber::getCurrent() still returns it, in that request's own
// shutdown functions and in every later request the worker serves on it.
//
// The engine catches the fatal inside the started fiber and raises it again in
// the request with its record of the current fiber cleared. The worker keeps the
// request's fiber and serves the next request on it, and the engine carries that
// record with the fiber across every switch — so whatever the worker leaves
// there after a fatal is what the fiber reports from then on. The cycle
// collector reads the same record: with none, it calls destructors on the
// request's own stack instead of in a fiber of its own.
//
// Three inner requests, one after the other, while this one is parked on each.
// They have to land on the same fiber for this to ask anything, and the request
// that fatals reports its fiber before the fatal so that is checked rather than
// assumed.

$t = new TestCase('request_fiber_stays_current_after_a_fatal_in_a_fiber', 'fibers');

/** The JSON lines of an inner request's body, merged. */
$reports = static function (string $body): array {
    $merged = [];
    foreach (explode("\n", $body) as $line) {
        $decoded = json_decode($line, true);
        if (is_array($decoded)) {
            $merged += $decoded;
        }
    }

    return $merged;
};

$fixture = '/tests/fibers/fixture_fatal_in_a_fiber_current.php';

$before = $reports(fiber_inner_request("$fixture?step=ident"));
$fatalBody = fiber_inner_request("$fixture?step=fatal");
$fatal = $reports($fatalBody);
$after = $reports(fiber_inner_request("$fixture?step=ident"));

$t->meta('before', $before);
$t->meta('fatal', $fatalBody);
$t->meta('after', $after);

$fiber = $before['fiber'] ?? null;
$t->assertNotNull('the first request ran in a fiber', $fiber);
$t->assertSame('the request that fatals ran in the same one', $fatal['fiber'] ?? null, $fiber);

// The fatal came from inside the started fiber, and nothing after it ran.
$t->assertContains(
    'the started fiber fataled',
    (string) ($fatal['last_error'] ?? ''),
    'OxphpFatalInAFiberCurrent'
);
$t->assertNotContains('and the request did not get past it', $fatalBody, 'NOT-REACHED');

$t->assertSame(
    'the request\'s shutdown functions still run in its fiber',
    $fatal['shutdown_fiber'] ?? null,
    $fiber
);
$t->assertSame('the next request on that fiber is told it runs in it', $after['fiber'] ?? null, $fiber);

// The collector's half, asked of a request served after the fatal. The first
// request answers it too, so a collector that never used a fiber of its own
// fails here before the fatal has had a chance to matter.
foreach (['before the fatal' => $before, 'after the fatal' => $after] as $when => $report) {
    $t->assertTrue("$when: the collector called the destructor", ($report['destructed'] ?? false) === true);
    $t->assertNotNull("$when: in a fiber", $report['destructor_fiber'] ?? null);
    $t->assertTrue(
        "$when: other than the request's",
        ($report['destructor_fiber'] ?? null) !== ($report['fiber'] ?? null)
    );
}

$t->done();
