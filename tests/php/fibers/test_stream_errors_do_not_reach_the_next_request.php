<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Leaves a stored stream error for the next request on this worker, which
// test_stream_errors_do_not_reach_the_next_request_probe then must not read: the
// engine only clears them in php_request_shutdown(), which a worker does not
// reach between requests. Before PHP 8.6 nothing is stored.

$t = new TestCase('stream_errors_do_not_reach_the_next_request', 'fibers');

if (function_exists('stream_last_errors')) {
    fopen('/nonexistent/oxphp-left-behind', 'r', false, stream_context_create(['stream' => ['error_mode' => StreamErrorMode::Silent]]));
    $t->assertSame('this request leaves one stored error', count(stream_last_errors()), 1);
} else {
    $t->assertTrue('nothing is stored before PHP 8.6', true);
}

$t->done();
