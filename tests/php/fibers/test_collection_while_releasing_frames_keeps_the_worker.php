<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// A destructor that runs while the worker gives back what an ended request's
// frames held, and asks for a cycle collection there, gets a collection that
// runs to its end — and the worker is kept.
//
// The engine raises the collector's guard for the whole of the bailout, and the
// collector checks it only after it has called the destructors of what it
// found: under the guard such a collection stops there and stays marked as
// running, which is just what a collection a fatal interrupted looks like. The
// worker lowers the guard before that give-back, so the collection finishes.
//
// The inner request is a stream whose client leaves while it is parked, so the
// write it resumes into ends it (see fibers/test_write_cancel_releases_frames).
// Right before that write it leaves a cycle with destructors for the collector.
// What the collection saw is read again by the probe on the next line, which
// would run on a replacement worker if this one were retired.

$t = new TestCase('collection_while_releasing_frames_keeps_the_worker', 'fibers');

OxphpBailoutLeak::arm('collect_in_release');
if (write_cancel_stage($t, 'stream-collects', '?in=stream-collects')) {
    $ran = write_cancel_wait(
        static fn (): bool => (OxphpBailoutLeak::state('collect_in_release')['gc'] ?? null) !== null,
        6.0
    );
    $t->assertTrue('the destructor the give-back ran asked for a collection', $ran);
}

$t->done();
