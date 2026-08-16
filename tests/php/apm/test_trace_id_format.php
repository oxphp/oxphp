<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('trace_id_format', 'apm');

$traceId = oxphp_apm_trace_id();
$t->assertMatch('trace id is 32 hex chars', $traceId, '/^[0-9a-f]{32}$/');

$t->done();
