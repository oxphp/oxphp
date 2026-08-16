<?php

// Reads, and on request seeds, one key of the per-worker shared array, and on
// request ends in a fatal afterwards.
//
// $sharedState is what makes this fixture the one the test needs: the worker's
// own handler carries a variable of that name too, so running this file hands
// the value from that frame to this one and leaves the handler's frame holding a
// copy it no longer owns. A fatal here abandons both frames at once, which is
// the state the release path has to unwind without giving the value up twice.
if (($_GET['seed'] ?? '') === '1') {
    $sharedState['fatal_probe'] = 'kept';
}

echo 'probe:' . ($sharedState['fatal_probe'] ?? 'gone');

if (($_GET['fatal'] ?? '') === '1') {
    // A fatal, not an exception: most of what reads as fatal in PHP 8 — an
    // undefined function, a type error — throws instead, and an exception
    // unwinds the frames on its way out rather than leaving them standing,
    // which is the state this is about. Nor trigger_error(E_USER_ERROR): it is
    // deprecated as of 8.4, and a test running before this one on the same
    // worker leaves an error handler installed that turns the deprecation into
    // a thrown exception, so the fatal never happens. Asking for more memory
    // than the limit allows is a fatal the engine raises itself, which no
    // handler is consulted about — and it costs nothing, because the limit is
    // checked before anything is allocated.
    $s = str_repeat('x', 512 * 1024 * 1024);
    echo strlen($s);
}
