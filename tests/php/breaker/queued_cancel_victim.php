<?php

declare(strict_types=1);

// A request whose client left while it waited in the queue, staged by
// queued_cancel_stage(). It must never run: its client is gone, and running it
// spends the worker on nobody while the requests queued behind it wait.
//
// The mark is the first thing it does, ahead of any output. A request that is
// run in spite of its client having left dies at its first write, so a mark
// placed after one would say nothing about whether it was run. The path is the
// one queued_cancel_marker() returns, written out: this file declares nothing,
// because on a build that runs it, it runs three times on the same worker.

file_put_contents('/tmp/oxphp-breaker-queued-cancel-victim', "ran\n", FILE_APPEND);

echo "unreachable: a request whose client left while it was queued must not be run\n";
