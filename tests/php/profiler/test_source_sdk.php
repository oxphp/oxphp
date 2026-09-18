<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// No trigger at all: the request arrives unprofiled and PHP turns profiling on
// from userland. This is the documented "no header needed" path, and it is the
// one activation the ActivationSource enum had no name for — every run started
// this way used to be filed under Header.
//
// profiler/test_source_index reads the resulting run back off index.json.

$t = new TestCase('source_sdk', 'profiler');

$t->assertFalse('no trigger on this request', OxPHP\Profile\is_active());

OxPHP\Profile\start();
$t->assertTrue('start() activated profiling', OxPHP\Profile\is_active());

function _src_sdk_leaf(int $n): int { return $n + 1; }
$t->assertSame('work ran under the profiler', _src_sdk_leaf(1), 2);

$t->done();
