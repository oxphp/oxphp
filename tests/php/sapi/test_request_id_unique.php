<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('request_id_unique', 'sapi');

$id = oxphp_request_id();
$t->assertMatch('oxphp_request_id() is a valid 20-hex string', $id, '/^[0-9a-f]{20}$/');

$t->meta('request_id', $id);

$t->done();
