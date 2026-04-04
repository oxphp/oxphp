<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('request_id', 'sapi');

$id = oxphp_request_id();
$t->assertMatch('oxphp_request_id() matches /^[0-9a-f]{20}$/', $id, '/^[0-9a-f]{20}$/');

$t->done();
