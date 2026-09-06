<?php

declare(strict_types=1);

// A fatal on a request whose client has already gone.
//
// This is the boundary of the case above it. What ends a request cancelled on
// the write path is a bailout carrying no message, and the mark that keeps such
// a request neutral for the breaker sits immediately in front of that bailout —
// but the message a fatal displays on its way out is a write like any other, so
// a fatal raised while the cancel cell is set reaches that mark first, before
// anything on the request says it is dying. Taking it for a cancellation would
// file the request as one the server ended, and an application fataling on every
// request would then keep its worker for as long as clients kept hanging up. A
// fatal is not put right by the client leaving afterwards: the engine has
// abandoned frames on the VM stack either way, and the next request on this
// worker inherits them.
//
// ignore_user_abort(true) plus a sleep longer than the suite line's --max-time
// is the deterministic way to reach the fatal with the cell already set: the
// interrupt handler records the disconnect for such a request and returns
// without unwinding it. Nothing may write between the sleep and the fatal — an
// echo there would be ended by the cancellation check itself, and the fatal
// would never be raised at all.
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
