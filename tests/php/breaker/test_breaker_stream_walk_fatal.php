<?php

declare(strict_types=1);

require_once __DIR__ . '/breaker_probe.php';
require_once __DIR__ . '/stream_walk.php';

// The same three streams, holding an object whose destructor raises a fatal
// instead. That fatal leaves the worker's engine state what any fatal leaves
// it — every object alive on it flagged as already destructed, so none of them
// ever runs its destructor again — and the client having left first does not
// put that back. Each of the three is a failure, and the third retires the
// worker.
//
// This request is the casualty and that is the assertion, as it is for
// test_breaker_eventloop_path: it is still parked when the worker leaves its
// loop, so it is answered 503. A build that files the three as cancellations
// leaves it to wake up and answer 200.
//
// No TestCase: this request does not get to report anything. What the probe
// after it needs from here is written to a file first.

$recycles = breaker_recycles();
file_put_contents(
    '/tmp/oxphp-breaker-walk-fatal-baseline',
    $recycles === null ? '' : json_encode($recycles)
);

// Written only when a step fails. The step that succeeds last is the one that
// retires the worker, so on a working build this request never gets back from
// it to say so.
$failedFile = '/tmp/oxphp-breaker-walk-fatal-staging-failed';
if (is_file($failedFile)) {
    unlink($failedFile);
}
stream_walk_clear('fatal');
for ($id = 1; $id <= STREAM_WALK_COUNT; $id++) {
    // The last one parks through its fixture's park rather than polling; see
    // stream_walk_stage().
    $failed = stream_walk_stage('fatal', $id, $id === STREAM_WALK_COUNT);
    if ($failed !== null) {
        file_put_contents($failedFile, $failed);
        break;
    }
}

echo "unreachable: three fatals in the give-back must retire this worker\n";
