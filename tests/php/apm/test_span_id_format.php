<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('span_id_format', 'apm');

// Must have an active span for span_id to return a value
$sid = oxphp_apm_start('test_span');
$spanId = oxphp_apm_span_id();
oxphp_apm_end($sid);
$t->assertMatch('span id is 16 hex chars', $spanId, '/^[0-9a-f]{16}$/');

$t->done();
