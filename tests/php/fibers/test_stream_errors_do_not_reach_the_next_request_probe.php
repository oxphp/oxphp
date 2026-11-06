<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The second half of test_stream_errors_do_not_reach_the_next_request. No request
// was parked between the two, which is the path a parked neighbour's test cannot
// take.

$t = new TestCase('stream_errors_do_not_reach_the_next_request_probe', 'fibers');

$t->assertSame(
    'this request starts with no stored stream error',
    function_exists('stream_last_errors') ? count(stream_last_errors()) : 0,
    0
);

$t->done();
