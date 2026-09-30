<?php

declare(strict_types=1);

// Served in its own request fiber while the request that fired it is parked in a
// read on the other end of a local socket. Waits a second, then writes the answer
// to the end the firing request left in the worker's shared state.
//
// The firing request's own read can only be answered by this one, which runs on
// the very worker thread that read belongs to: PHP_WORKERS=1 is what makes the
// difference between "the read parked" and "the read held the thread" visible.
$which = $_GET['which'] ?? '';
$writer = $sharedState["net_{$which}_writer"] ?? null;
if (!is_resource($writer)) {
    echo 'inner-no-writer';
    return;
}

sleep(1);
fwrite($writer, 'net-done');
echo 'inner-done';
