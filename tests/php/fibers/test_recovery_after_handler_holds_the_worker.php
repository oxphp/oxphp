<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recovery_probe.php';

// A decorator's after() that sleeps while the worker recovers from a fatal
// sleeps on the worker's thread, and no other request runs until it is done.
//
// The recovery runs with the fatal's marks still on the thread — the engine's
// note that the shutdown is unclean, the cycle collector held off — and lowers
// them only at its end. They are the thread's, not the request's: a request
// handed the thread meanwhile reads them as a fatal of its own. So the sleep
// must not park. This request polls while the inner one sleeps, and seeing the
// inner one mid-sleep is the thread having been handed over.

$t = new TestCase('recovery_after_handler_holds_the_worker', 'fibers');

OxphpRecoveryProbe::reset();
$sock = recovery_inner_send('/tests/fibers/fixture_recovery_after_handler.php?case=sleep');
$t->assertTrue('inner request socket connected', $sock !== false);

if ($sock !== false) {
    $sawWindow = false;
    $deadline = microtime(true) + 5.0;
    while (microtime(true) < $deadline) {
        $stage = OxphpRecoveryProbe::$stage;
        if ($stage === 'after-sleeping') {
            $sawWindow = true;
        }
        if ($stage === 'after-slept') {
            break;
        }
        // Hooked: parks this request, which is what lets the inner one run.
        usleep(20_000);
    }
    $resp = recovery_inner_read($sock);
    $t->meta('response', $resp);

    // Without these the last assertion proves nothing: an after() that never
    // ran, or never got to its sleep, has no window to be seen in.
    $t->assertContains('the inner request fataled', $resp, ' 500 ');
    $t->assertSame(
        "the decorator's after() ran in the recovery and slept to its end",
        OxphpRecoveryProbe::$stage,
        'after-slept'
    );
    $t->assertFalse('no other request ran while it slept', $sawWindow);
}

$t->done();
