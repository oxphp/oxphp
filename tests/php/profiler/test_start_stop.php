<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

$t = new TestCase('start_stop', 'profiler');

// Initially not active.
$t->assertFalse('initially inactive', OxPHP\Profile\is_active());

// Start activates.
OxPHP\Profile\start();
$t->assertTrue('start() activates', OxPHP\Profile\is_active());

// Idempotent — calling start() twice is harmless.
OxPHP\Profile\start();
$t->assertTrue('start() is idempotent', OxPHP\Profile\is_active());

// Stop deactivates.
OxPHP\Profile\stop();
$t->assertFalse('stop() deactivates', OxPHP\Profile\is_active());

// Idempotent.
OxPHP\Profile\stop();
$t->assertFalse('stop() is idempotent', OxPHP\Profile\is_active());

// Re-start after stop works.
OxPHP\Profile\start();
$t->assertTrue('re-start after stop works', OxPHP\Profile\is_active());

OxPHP\Profile\stop();
$t->done();
