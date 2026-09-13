<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/queued_cancel.php';

// The three requests staged on the previous line were not run.
//
// Their clients were gone before the worker reached them. Running them anyway
// costs the worker whatever each handler does before its first write — for an
// application that queries a database and then renders, all of it — while the
// requests queued behind them wait, and nobody is there to read the result.

$test = new TestCase('breaker_queued_cancel_skipped', 'breaker');

$state = '/tmp/oxphp-breaker-queued-cancel-stage';
$test->assertTrue('the staging on the previous line completed', is_file($state));

$m = queued_cancel_metrics();
$test->assertNotNull('/metrics is readable', $m);

if (is_file($state) && $m !== null) {
    $before = json_decode((string) file_get_contents($state), true);

    // This request was queued behind the three, so the worker did reach them.
    $test->assertSame('the queue is empty', $m['queue_depth'], 0);

    $test->assertSame('none of the three was run', queued_cancel_victims_run(), 0);

    // Nor counted as served, by either count: the worker's own has the staging
    // request and this one, and the server's has the staging request, this one
    // not having been answered yet.
    $test->assertSame(
        'the worker counts the staging request and this one',
        OxPHP\Server\Worker::current()->requestCount(),
        $before['request_count'] + 1
    );
    $test->assertSame(
        'the server counts the staging request as handled, and nothing else',
        $m['handled'],
        $before['handled'] + 1
    );
}

$test->done();
