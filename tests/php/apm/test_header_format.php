<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('header_format', 'apm');

// Must have an active span for header to include span_id
$sid = oxphp_apm_start('test_span');
$header = oxphp_apm_header();
oxphp_apm_end($sid);
$t->assertMatch(
    'traceparent header matches W3C format',
    $header,
    '/^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/'
);

$t->done();
