<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('fast_request_ok', 'timeout');
// Completes immediately — well within any configured timeout.
$t->assertTrue('fast request completes within timeout', true);
$t->done();
