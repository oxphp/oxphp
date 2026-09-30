<?php

declare(strict_types=1);

// Served in its own request fiber while the request that fired it is parked in a
// write that cannot proceed: the socket's buffer is full and the far end, which
// this request holds, is not being read. Waits a second, notes when it ran, then
// reads the far end dry.
//
// It can only run on the worker thread the writer belongs to, which PHP_WORKERS=1
// makes the only one, so the time it records says whether that write parked or held
// the thread.
$reader = $sharedState['net_drain_reader'] ?? null;
if (!is_resource($reader)) {
    echo 'inner-no-reader';
    return;
}

sleep(1);
$sharedState['net_drain_ran_at'] = microtime(true);

stream_set_blocking($reader, false);
$drained = 0;
while (($chunk = fread($reader, 65536)) !== false && $chunk !== '') {
    $drained += strlen($chunk);
}
$sharedState['net_drain_bytes'] = $drained;
echo 'inner-done';
