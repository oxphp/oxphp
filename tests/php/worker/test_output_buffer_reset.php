<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('output_buffer_reset', 'worker');

// The output buffer level at the start of each request should be clean (0 or 1,
// depending on whether the SAPI wraps in an ob level, but never > 1 from leakage).
$level = ob_get_level();
$t->assertTrue('ob_get_level() is 0 or 1 at request start', $level <= 1);

$t->done();
