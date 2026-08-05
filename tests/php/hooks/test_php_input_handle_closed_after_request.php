<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The php://input wrapper holds the body it reads through as a raw pointer and
// counts no references to it. The body belongs to the request and is closed when
// that request ends, so a handle a request leaves standing in worker-scope state
// — a static, a global, a PSR-7 ServerRequest whose body is fopen('php://input')
// cached in a container built once per worker — would be reading freed memory on
// every request after it. The end of a request has to close the handle as well
// as what it points at, which turns that into PHP's own "supplied resource is
// not a valid stream resource" at the point of misuse.
//
// $sharedState is the per-worker store the worker fixture keeps for exactly this
// shape; PHP `include` runs in the includer's scope, so this file reaches it.
// Two sightings, one worker (PHP_WORKERS=1 in this profile): the first request
// leaves the handle there, the second finds it.

$t = new TestCase('php_input_handle_closed_after_request', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

if ($sighting === 1) {
    $sharedState['php_input_handle'] = fopen('php://input', 'r');
    $t->assertTrue(
        'the handle is a live stream inside the request that opened it',
        is_resource($sharedState['php_input_handle'])
    );
    $t->assertNotEmpty(
        'and it reads that request\'s own body',
        (string) stream_get_contents($sharedState['php_input_handle'])
    );
} else {
    $t->assertKeyExists(
        'the request before this one left its handle in worker-scope state',
        $sharedState,
        'php_input_handle'
    );
    // Asked about rather than read from. On a build that leaves the handle open,
    // reading it is a use-after-free of the body its request already closed, and
    // a test must not perform one to show that it is possible.
    $t->assertFalse(
        'a php://input handle that outlived its request is closed, not left dangling',
        is_resource($sharedState['php_input_handle'] ?? null)
    );
    unset($sharedState['php_input_handle']);
}

$t->done();
