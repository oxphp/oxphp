<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('pause_resume', 'profiler');

OxPHP\Profile\start();
$t->assertTrue('active after start', OxPHP\Profile\is_active());

OxPHP\Profile\pause();
$t->assertFalse('inactive while paused', OxPHP\Profile\is_active());

OxPHP\Profile\resume();
$t->assertTrue('active after resume', OxPHP\Profile\is_active());

// pause+pause+resume — single resume clears.
OxPHP\Profile\pause();
OxPHP\Profile\pause();
OxPHP\Profile\resume();
$t->assertTrue('single resume clears multiple pauses', OxPHP\Profile\is_active());

OxPHP\Profile\stop();
$t->done();
