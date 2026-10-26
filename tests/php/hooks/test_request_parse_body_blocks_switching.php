<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../engine_fatal.php';

// request_parse_body() must hold fiber switching down for the whole call, as the
// worker's own parse of the request body does. Several of the fields a parse
// works through are one slot per worker thread and do not travel with a
// suspending request, and the parse calls userland mid-way: a body over a limit
// raises a warning, and the application's error handler runs. A handler that
// sleeps under runtime hooks would park the request there and hand the next one
// the pieces of this one's parse. The engine answers a Fiber that tries to
// switch under the block with FiberError, which is what the probe asks.
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile):
//   1. the handler sees switching blocked, and the call leaves it unblocked;
//   2. the handler bails out of the parse with a fatal;
//   3. switching works on the same worker — the fatal did not leave it blocked.
//
// The handler is installed for one call only. A worker keeps error handlers
// across requests, and the fatal in sighting 2 leaves its handler on the stack;
// it is disarmed through $sharedState before it fatals, so from then on it
// passes every error through.

$t = new TestCase('request_parse_body_blocks_switching', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$probe = static function (): string {
    try {
        $fiber = new \Fiber(static function (): string {
            return (string) \Fiber::suspend('suspended');
        });
        $first = $fiber->start();
        $fiber->resume('resumed');
        return $first . '/' . $fiber->getReturn();
    } catch (\FiberError $e) {
        return 'FiberError: ' . $e->getMessage();
    }
};

if ($sighting === 1) {
    $seen = [];
    set_error_handler(static function (int $errno, string $errstr) use (&$seen, $probe): bool {
        $seen[] = ['msg' => $errstr, 'probe' => $probe()];
        return true;
    });
    try {
        request_parse_body(['post_max_size' => 1]);
    } finally {
        restore_error_handler();
    }

    $t->meta('seen', $seen);
    // The premise: the handler ran inside the call, raised by the parse itself.
    $t->assertContains(
        'the parse called the error handler for the body over post_max_size',
        (string) ($seen[0]['msg'] ?? ''),
        'exceeds the limit of 1 bytes'
    );
    $t->assertSame(
        'and switching was blocked while the handler ran',
        $seen[0]['probe'] ?? null,
        'FiberError: Cannot switch fibers in current execution context'
    );
    $t->assertSame('after the call switching works again', $probe(), 'suspended/resumed');
} elseif ($sighting === 2) {
    $sharedState['request_parse_body_fatal_armed'] = true;
    set_error_handler(static function () use (&$sharedState): bool {
        if (($sharedState['request_parse_body_fatal_armed'] ?? false) !== true) {
            return false;
        }
        $sharedState['request_parse_body_fatal_armed'] = false;
        engine_fatal('OxphpFatalInRequestParseBody');
    });
    request_parse_body(['post_max_size' => 1]);
    restore_error_handler();
    $t->assertTrue('unreachable: the handler fatals inside the call', false);
} else {
    $t->assertSame(
        'the request before this one fatalled from inside the parse',
        $sharedState['request_parse_body_fatal_armed'] ?? null,
        false
    );
    unset($sharedState['request_parse_body_fatal_armed']);
    $t->assertSame('switching works on the worker that fatalled', $probe(), 'suspended/resumed');
}

$t->done();
