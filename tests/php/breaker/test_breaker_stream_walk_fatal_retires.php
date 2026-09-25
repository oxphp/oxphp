<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';
require_once __DIR__ . '/stream_walk.php';

// The worker that took three fatals in the give-back after a stream's write
// ended it is gone, and this request is answered by its replacement. See
// test_breaker_stream_walk_fatal.

$test = new TestCase('breaker_stream_walk_fatal_retires', 'breaker');

$test->assertNull(
    'no step of the staging failed',
    stream_walk_read('/tmp/oxphp-breaker-walk-fatal-staging-failed')
);

for ($id = 1; $id <= STREAM_WALK_COUNT; $id++) {
    // As in the throw block: the fatal was raised in the give-back, after the
    // write had ended the request, and not on some other way out.
    $test->assertSame(
        "stream $id: its destructor ran in the give-back after the write ended it",
        stream_walk_read(stream_walk_dtor_file('fatal', $id)),
        'before-write'
    );
}

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$baseline = json_decode((string) stream_walk_read('/tmp/oxphp-breaker-walk-fatal-baseline'), true);
$test->assertTrue('the staging recorded the recycles before it', is_array($baseline));
$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if (is_array($baseline) && $recycles !== null) {
    $test->assertSame('one more worker was recycled', $recycles['total'], $baseline['total'] + 1);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], $baseline['error'] + 1);
}

$test->done();
