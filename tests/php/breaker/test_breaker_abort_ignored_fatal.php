<?php

declare(strict_types=1);

// A fatal on a request whose client has already gone.
//
// This is the boundary of the case above it. The message a fatal displays on
// its way out is a write like any other, and it reaches the write path's
// cancellation check while the cancel cell holds the client's departure. In
// worker mode that check lets a request that is not streaming go on, so the
// fatal runs its course and is filed as a fatal; this pins that it stays so.
// Taking it for a cancellation would file the request as one the server ended,
// and an application fataling on every request would then keep its worker for
// as long as clients kept hanging up. A fatal is not put right by the client
// leaving first: the engine has abandoned frames on the VM stack either way,
// and the next request on this worker inherits them.
//
// A sleep longer than the suite line's --max-time is the deterministic way to
// reach the fatal with the cell already set: the interrupt handler records the
// disconnect and returns without unwinding the request. The
// ignore_user_abort(true) call is kept from when the setting was what let the
// request through; in worker mode, outside a stream, it no longer changes the
// outcome.
//
// The fatal is a class declared twice, bound at runtime so both declarations are
// executed rather than the file being rejected while it compiles — the same
// construction, and for the same reasons, as test_breaker_fatal. A call to an
// undefined function would not do: PHP 8 throws an Error for that, and an
// uncaught exception is neutral for the breaker by design.

ignore_user_abort(true);

// Native blocking sleep, as in test_breaker_abort_ignored: no runtime hooks in
// this profile, so the request stays on the worker's fast path.
usleep(2_000_000);

if (count($_SERVER) > 0) {
    class BreakerAbortIgnoredFatal
    {
    }
}

if (count($_SERVER) > 0) {
    class BreakerAbortIgnoredFatal
    {
    }
}

echo "unreachable: declaring a class twice must be fatal\n";
