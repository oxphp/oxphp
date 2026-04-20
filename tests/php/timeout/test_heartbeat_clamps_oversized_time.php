<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('heartbeat_clamps_oversized_time', 'timeout');

// Arm PHP's own timer at 1s. The clamp itself fires regardless, but
// keeping the PHP timer enabled lets the proof-of-life at the bottom
// confirm zend_set_timeout(INT_MAX) was actually called — a usleep past
// the original 1s limit would fatal otherwise.
set_time_limit(1);

// TestCase installs an error handler that converts every warning into an
// ErrorException, which would mask the clamp warning and abort the request.
// Swap it out for a collector so we can inspect the emitted message.
$warnings = [];
set_error_handler(function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = ['errno' => $errno, 'message' => $errstr];
    return true; // suppress default handling
});

// PHP_INT_MAX on 64-bit is 2^63-1, well past INT_MAX (2^31-1 = 2147483647).
// time * 1_000_000 for the server deadline would overflow int64, and
// zend_set_timeout() takes int — so $time is clamped to INT_MAX up front
// for both sides, and the user must be told via E_WARNING.
$result = oxphp_request_heartbeat(PHP_INT_MAX);

restore_error_handler();

$t->assertTrue('oxphp_request_heartbeat returns true despite clamp', $result);
$t->assertCount('exactly one warning emitted', $warnings, 1);
$t->assertSame('warning is E_WARNING', $warnings[0]['errno'] ?? null, E_WARNING);
$t->assertContains(
    'warning mentions clamp',
    $warnings[0]['message'] ?? '',
    'clamped to'
);
$t->assertContains(
    'warning mentions function name',
    $warnings[0]['message'] ?? '',
    'oxphp_request_heartbeat'
);
$t->assertContains(
    'warning mentions the oversized input',
    $warnings[0]['message'] ?? '',
    (string) PHP_INT_MAX
);

// Proof-of-life: both deadlines were (re)armed, so a short sleep past the
// original 1s PHP limit must not trigger "Maximum execution time exceeded".
usleep(1_200_000);
$t->assertTrue('request survived past original 1s PHP limit', true);

$t->done();
