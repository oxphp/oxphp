<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/queued_cancel.php';

// Three requests whose clients leave while they wait in the queue, reached by a
// worker with nothing else running.
//
// This request stages them and returns. It never yields, so the worker has no
// fiber to come back to once it is done, and it takes the three from the queue
// on its blocking branch — the branch that waits for the next request rather
// than ticking an event loop. The probe on the next line reads what it did with
// them.
//
// What this request writes down is what the probe compares against: the
// worker's own count and the server's count of requests the worker handled, as
// they stood before any of the three could have been taken.

$test = new TestCase('breaker_queued_cancel_stage', 'breaker');

$before = queued_cancel_stage($test);

$state = '/tmp/oxphp-breaker-queued-cancel-stage';
if (is_file($state)) {
    unlink($state);
}
if ($before !== null) {
    file_put_contents($state, json_encode([
        'request_count' => OxPHP\Server\Worker::current()->requestCount(),
        'handled' => $before['handled'],
    ]));
}

$test->done();
