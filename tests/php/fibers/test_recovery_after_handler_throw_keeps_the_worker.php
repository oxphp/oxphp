<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recovery_probe.php';

// A decorator's after() that throws while the worker recovers from a fatal
// costs the request that fataled, not the worker — even when dropping what it
// threw raises a fatal of its own.
//
// The exception after() leaves standing is dropped by the recovery, and
// dropping it runs its __destruct: the engine's "already destructed" mark is
// put on the objects that existed when the request fataled, and this one was
// built after. A fatal raised there, outside every guard the recovery holds,
// takes the bailout target of whatever called it — past the rest of the
// recovery and out of the loop that serves requests. This request is parked on
// the same worker while the inner one runs, so getting to its assertions on
// the worker it started on is what says the worker kept serving.

$t = new TestCase('recovery_after_handler_throw_keeps_the_worker', 'fibers');

$worker = OxPHP\Server\Worker::current();
$started = $worker->startTime();
$served = $worker->requestCount();

OxphpRecoveryProbe::reset();
$sock = recovery_inner_send('/tests/fibers/fixture_recovery_after_handler.php?case=throw');
$t->assertTrue('inner request socket connected', $sock !== false);

if ($sock !== false) {
    $resp = recovery_inner_read($sock);
    $t->meta('response', $resp);

    $t->assertContains('the inner request fataled', $resp, ' 500 ');
    // The premise: the exception after() threw was dropped and its destructor
    // ran, which is where the second fatal comes from.
    $t->assertSame(
        'the destructor of what after() threw ran',
        OxphpRecoveryProbe::$stage,
        'exception-destructing'
    );
}

$t->assertSame('the worker that ran it is still serving', OxPHP\Server\Worker::current()->startTime(), $started);
$t->assertGreaterThan('and served the inner request', OxPHP\Server\Worker::current()->requestCount(), $served);

$t->done();
