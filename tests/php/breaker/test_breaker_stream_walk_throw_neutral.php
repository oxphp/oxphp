<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';
require_once __DIR__ . '/stream_walk.php';

// Three streams whose clients left while they were parked, each ended at the
// write it resumed into, each holding an object whose destructor throws as the
// worker gives back what the ended request abandoned. The throw is reported as
// an uncaught exception, which is an application outcome: the request stays a
// cancellation, and three of them do not retire the worker.
//
// It is also this request's own survival that says so. It is parked while the
// three run on the same worker, and a worker retired underneath it would end it
// before it reached a single assertion below.

$test = new TestCase('breaker_stream_walk_throw_neutral', 'breaker');

$recyclesBefore = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recyclesBefore);

stream_walk_clear('throw');
for ($id = 1; $id <= STREAM_WALK_COUNT; $id++) {
    $test->assertNull("stream $id: every step of the staging happened", stream_walk_stage('throw', $id));
}

for ($id = 1; $id <= STREAM_WALK_COUNT; $id++) {
    // Destructed after the request reached its write and before it got past it:
    // the write ended the request, and the destructor ran in the give-back. One
    // that ran at 'parked' was released some other way, and one at
    // 'after-write' by the request's own return.
    $test->assertSame(
        "stream $id: its destructor ran in the give-back after the write ended it",
        stream_walk_read(stream_walk_dtor_file('throw', $id)),
        'before-write'
    );
}

$recycles = breaker_recycles();
if ($recyclesBefore !== null && $recycles !== null) {
    $test->assertSame('no worker was recycled', $recycles['total'], $recyclesBefore['total']);
}

$test->done();
