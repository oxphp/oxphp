<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('is_active', 'profiler');

// Without a trigger header / cookie / query, this request did not
// activate ProfileAll, so is_active() must return false.
$t->assertFalse('is_active() returns false without trigger',
    OxPHP\Profile\is_active());

// After OxPHP\Profile\start() promotes the bridge mode, is_active()
// returns true mid-request.
OxPHP\Profile\start();
$t->assertTrue('is_active() returns true after start()',
    OxPHP\Profile\is_active());

// stop() pauses capture; is_active() returns false.
OxPHP\Profile\stop();
$t->assertFalse('is_active() returns false after stop()',
    OxPHP\Profile\is_active());

$t->done();
