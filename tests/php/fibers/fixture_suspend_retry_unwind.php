<?php

declare(strict_types=1);

// A request that keeps retrying Fiber::suspend() until the server ends it, and
// whose unwind then suspends the fiber again. Deliberately not a TestCase — it
// is the subject of test_suspend_retry_end_waits_for_unwind, which checks the
// marker it leaves to see when the unwind finished.
//
//   mode=park    its finally block parks on a timer before leaving the marker
//   mode=escape  its finally block throws, a catch further out takes that and
//                suspends again; it leaves the marker once its loop runs out
//   mode=plain   answers at once: the request that runs after one of the above
header('Content-Type: text/plain');

$mode = (string) ($_GET['mode'] ?? 'plain');
$marker = sys_get_temp_dir() . '/oxphp-req-suspend-' . preg_replace('/[^0-9a-f]/', '', (string) ($_GET['id'] ?? ''));

if ($mode === 'park') {
    try {
        while (true) {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            }
        }
    } finally {
        oxphp_sleep(0.05);
        file_put_contents($marker, 'ran');
    }
}

if ($mode === 'escape') {
    for ($i = 0; $i < 100; $i++) {
        try {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            } finally {
                throw new \RuntimeException('thrown from finally');
            }
        } catch (\RuntimeException) {
        }
    }
    file_put_contents($marker, 'ran');
}

echo "handler ran to its end: {$mode}\n";
