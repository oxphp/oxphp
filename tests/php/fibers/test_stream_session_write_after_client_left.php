<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';
require_once __DIR__ . '/../breaker/breaker_probe.php';

// A stream whose client left, ended by its save handler's output as the
// session is written, is a cancellation — not a request that came apart.
//
// Three in a row, all while this request is parked on the same worker. Each is
// ended by a write, and a write-ended request is filed by a claim the write
// raises: read as the handler failing, three of them would reach the
// consecutive-error threshold, and the worker would leave its loop there —
// unwinding this request with it, so this test would never report. Nothing
// between the three resets that run: this request does not finish until after
// them.
//
// Each stage reads both edges: the save handler got to its write, and did not
// get past it.

$t = new TestCase('stream_session_write_after_client_left', 'fibers');

$before = breaker_recycles();
$t->assertNotNull('/metrics exposes the worker-mode block', $before);

for ($i = 1; $i <= 3; $i++) {
    if (!write_cancel_stage($t, "leave $i", '', 'fixture_stream_session_write.php')) {
        continue;
    }

    $wrote = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'session-write'
            || OxphpWriteCancelProbe::$stage === 'past-session-write',
        4.0
    );
    $t->assertTrue("leave $i: the save handler wrote after its client had left", $wrote);
    $t->assertSame("leave $i: and that write ended the request", OxphpWriteCancelProbe::$stage, 'session-write');
}

$after = breaker_recycles();
if ($before !== null && $after !== null) {
    $t->assertSame('no worker was retired for it', $after['total'], $before['total']);
}

$t->done();
