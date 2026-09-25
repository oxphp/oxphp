<?php

declare(strict_types=1);

require_once __DIR__ . '/breaker_probe.php';
require_once __DIR__ . '/stream_walk.php';

// Three streams that return without writing after their client left, and whose
// session save handler then writes as the worker writes the session. That
// write is what ends each of them, past the request's own end, and the object
// the save handler holds is given back from its frame — where its destructor
// raises a fatal. The write's ending would make the request a cancellation; the
// fatal raised while its frames are given back comes after it and stands
// against it, as it does for a stream ended inside its handler. Three retire the
// worker.
//
// This request is the casualty and that is the assertion, as it is for
// test_breaker_stream_walk_fatal: answered 503 when the worker leaves its loop,
// 200 by a build that files the three as cancellations.
//
// No TestCase: this request does not get to report anything. What the probe
// after it needs from here is written to a file first.

$recycles = breaker_recycles();
file_put_contents(
    '/tmp/oxphp-breaker-walk-session-baseline',
    $recycles === null ? '' : json_encode($recycles)
);

// Written only when a step fails; see test_breaker_stream_walk_fatal.
$failedFile = '/tmp/oxphp-breaker-walk-session-staging-failed';
if (is_file($failedFile)) {
    unlink($failedFile);
}
// Ids of their own, so none of these markers is one the fatal block wrote.
$ids = [7, 8, 9];
stream_walk_clear('fatal', $ids);
foreach ($ids as $id) {
    // The last one parks through its fixture's park rather than polling; see
    // stream_walk_stage().
    $failed = stream_walk_stage('fatal', $id, $id === 9, 'session');
    if ($failed !== null) {
        file_put_contents($failedFile, $failed);
        break;
    }
}

echo "unreachable: three fatals in the give-back must retire this worker\n";
