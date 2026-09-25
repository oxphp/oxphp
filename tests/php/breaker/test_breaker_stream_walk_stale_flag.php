<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';
require_once __DIR__ . '/stream_walk.php';

// A fatal, a throw, a fatal — three streams on one worker, each ended at its
// write with a destructor to run in the give-back. What decides whether a walk
// raised a fatal is noted per worker thread, so the note the first stream's
// fatal leaves is still there when the second stream's walk starts. That walk
// only throws: it has to be judged by what it raised itself, and stay a
// cancellation. Then the two fatals are two failures, not three in a row, and
// the worker stays.
//
// This request's own survival is the assertion, as in the throw block: had the
// throw been counted with the fatals, the worker would have been retired under
// it and it would have been answered 503.

$test = new TestCase('breaker_stream_walk_stale_flag', 'breaker');

$recyclesBefore = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recyclesBefore);

// Ids of their own, so none of these markers is one the blocks above wrote.
$streams = [4 => 'fatal', 5 => 'throw', 6 => 'fatal'];
stream_walk_clear('fatal', [4, 6]);
stream_walk_clear('throw', [5]);
foreach ($streams as $id => $dtor) {
    // The last parks through its fixture's park; see stream_walk_stage(). A
    // build that counts the throw retires the worker at that one, and polling
    // could let this request finish in the same tick and clear the count first.
    $test->assertNull(
        "stream $id ($dtor): every step of the staging happened",
        stream_walk_stage($dtor, $id, $id === 6)
    );
}

foreach ($streams as $id => $dtor) {
    $test->assertSame(
        "stream $id ($dtor): its destructor ran in the give-back after the write ended it",
        stream_walk_read(stream_walk_dtor_file($dtor, $id)),
        'before-write'
    );
}

$recycles = breaker_recycles();
if ($recyclesBefore !== null && $recycles !== null) {
    $test->assertSame('no worker was recycled', $recycles['total'], $recyclesBefore['total']);
}

$test->done();
