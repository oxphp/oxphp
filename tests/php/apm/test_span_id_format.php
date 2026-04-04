<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('span_id_format', 'apm');

$spanId = oxphp_apm_span_id();
$t->assertMatch('span id is 16 hex chars', $spanId, '/^[0-9a-f]{16}$/');

$t->done();
