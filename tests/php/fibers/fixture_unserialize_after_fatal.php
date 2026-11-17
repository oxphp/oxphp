<?php

declare(strict_types=1);

// Inner request for fibers/test_unserialize_state_after_a_fatal.
//
// ?role=fail fails inside unserialize() twice. First in the handler: a warning
// the parse raises is turned into a fatal by the error handler while the
// unserialize() call is still open, which leaves the engine's count of open
// calls raised. A shutdown function then records what unserialize() does at
// that point, and stays busy long enough for the probe the test sends to be
// queued behind this request. A second shutdown function fails the same way,
// which leaves the count raised past this request's end.
//
// ?role=probe records what unserialize() does in the request that starts next.

require_once __DIR__ . '/thread_cell_probe.php';

ini_set('display_errors', '0');

if (($_GET['role'] ?? '') === 'probe') {
    $sharedState['cell_unser_next'] = thread_cell_unserialize_probe();
    echo 'probed';

    return;
}

register_shutdown_function(static function () use (&$sharedState): void {
    $sharedState['cell_unser_shutdown'] = thread_cell_unserialize_probe();
    // Busy rather than asleep: a park would take this request's state off the
    // thread, and the probe has to find what this request leaves standing.
    $until = microtime(true) + 0.3;
    while (microtime(true) < $until) {
    }
});
register_shutdown_function(static function (): void {
    set_error_handler(static function (): bool {
        trigger_error('fatal inside unserialize() in a shutdown function', E_USER_ERROR);
    });
    unserialize('garbage');
});

set_error_handler(static function (): bool {
    trigger_error('fatal inside unserialize() in the handler', E_USER_ERROR);
});
unserialize('garbage');

echo 'not reached';
