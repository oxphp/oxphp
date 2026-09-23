<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/cancel_finally_guard.php';

// A worker-mode request whose client leaves while it is parked runs its
// userland `finally` — it is not ended at the write it resumes into.
//
// Ending a request from inside a write is a bailout, and a bailout is not an
// unwind: it jumps out without re-entering the engine, and `finally` is reached
// only from inside it. The frames it abandons are walked afterwards and their
// locals released, so even a destructor still runs, late; `finally` has nothing
// that does the same for it. In worker mode the request is not what is ending —
// the worker carries on, and whatever the abandoned request had marked on it
// would still be marked, for every request after it, for the rest of that
// worker's life. Code that guards a render against containing itself would then
// hold a guard against a render that finished minutes ago, and refuse the real
// one. So a worker-mode request that is not streaming is not ended at that
// write: it runs to its end, `finally` included.
//
// Were it ended there, the damage would be silent and permanent: nothing is
// logged, no status changes, and no later request clears a static it did not
// set. The visible effect would be a worker that quietly stops rendering the
// guarded thing, and the only way to see it is to go and look at the mark —
// which is what this does.
//
// Both edges of the window are read. The mark has to be up while the inner
// request is parked on it, or the run proved nothing about the mark coming
// down; and it has to be down once that request is over.

$t = new TestCase('cancel_runs_userland_cleanup', 'fibers');

// Unique per run: the mark is worker state, and a mark left by an earlier run
// of this same test would otherwise satisfy the first edge on its own.
$key = 'r' . bin2hex(random_bytes(4));

$t->assertFalse('the worker carries no mark under this key yet', isset(cancel_finally_guard_seen()[$key]));

$staged = write_cancel_stage($t, 'guarded write', '?key=' . $key, 'fixture_cancel_runs_finally.php');

if ($staged) {
    // The first edge. Read from this request, which is multiplexed onto the
    // same worker as the inner one and so reads the same per-thread table.
    // Without it the last assertion is satisfied by a run in which the inner
    // request never entered the guarded window at all.
    $t->assertTrue(
        'the inner request marked the worker before it parked',
        isset(cancel_finally_guard_seen()[$key])
    );

    // It resumes out of the park and writes to a client that has gone, which
    // does not end it. Checked separately from the mark so that a run in which the
    // inner request never resumed is reported as that, and not as a leak.
    $reached = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage !== 'parked',
        6.0
    );
    $t->assertTrue('the inner request resumed out of its park', $reached);

    // The second edge. Polled rather than read once: the inner request goes on
    // past the write to its own end, and this request is parked in the poll's
    // own sleep while it does.
    $unmarked = write_cancel_wait(
        static fn (): bool => !isset(cancel_finally_guard_seen()[$key]),
        6.0
    );
    $t->assertTrue(
        'the cancelled request ran its finally and left no mark on the worker',
        $unmarked
    );

    // Corroboration, and the line that says which half failed: 'cleaned' is set
    // by the `finally` itself, so a run that ends on 'before-write' is one where
    // the block was left without it running.
    $t->assertSame(
        'the guarded block was left through its finally',
        OxphpWriteCancelProbe::$stage,
        'cleaned'
    );

    // Letting the request finish is only half of it: the script is now the one
    // deciding when to stop, so it has to be able to see that there is nothing
    // left to stop for. This request was parked when its client left, which is
    // the case that reaches the write without ever passing an opcode boundary
    // carrying the interrupt — the half that used to be the only one marking
    // the connection.
    $t->assertSame(
        'and could see its client had gone',
        OxphpWriteCancelProbe::$abortedAfterWrite,
        1
    );
}

$t->done();
