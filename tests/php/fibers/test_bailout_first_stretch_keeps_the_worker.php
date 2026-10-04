<?php

declare(strict_types=1);

// A new worker's first request, which builds up more than the threshold of what
// it keeps on purpose and is ended by a fatal inside an internal function, does
// not retire the worker.
//
// What a worker holds after its first requests is mostly what the application
// built on demand — a container, the metadata of an ORM — and the heap alone
// cannot say that apart from what a call left behind when a fatal ended the
// request inside an internal function. Judging that first stretch would retire
// a worker for its own warm-up, and its replacement would meet the same thing
// again for as long as the trouble lasted.
//
// Here the growth is the application's own: more than the threshold, kept on
// purpose before the fatal.
//
// The fatal is the engine's memory limit, which no error handler an earlier
// request left installed can swallow, and which is raised inside str_repeat().

require_once __DIR__ . '/bailout_leak_probe.php';

$replaced = OxphpBailoutLeak::state('replaced');
OxphpBailoutLeak::arm('first_stretch', [
    // Its own worker, and the first request on it: anything else makes what
    // follows a test of something other than the first stretch.
    'fresh' => OxPHP\Server\Worker::current()->requestCount() === 1
        && OxphpBailoutLeak::worker() !== ($replaced['worker'] ?? null),
]);

OxphpBailoutLeak::$retained[] = str_repeat('w', OxphpBailoutLeak::RETIRE_BYTES + 1024 * 1024);

ini_set('memory_limit', '64M');
str_repeat('x', 128 * 1024 * 1024);

echo 'finished';
