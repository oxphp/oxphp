<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

$t = new TestCase('metric', 'profiler');

// Silent before start.
OxPHP\Profile\metric('orphan', 1.0);
$t->assertTrue('metric before start completed without exception', true);

OxPHP\Profile\start();

// Integer-valued.
OxPHP\Profile\metric('rows', 1234.0);
$t->assertTrue('integer metric completed without exception', true);

// Float-valued.
OxPHP\Profile\metric('ratio', 0.875);
$t->assertTrue('float metric completed without exception', true);

// Zero.
OxPHP\Profile\metric('zero', 0.0);
$t->assertTrue('zero metric completed without exception', true);

// Negative.
OxPHP\Profile\metric('delta', -42.5);
$t->assertTrue('negative metric completed without exception', true);

OxPHP\Profile\stop();
$t->done();
