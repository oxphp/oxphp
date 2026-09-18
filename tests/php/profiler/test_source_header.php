<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Activated by the X-OxPHP-Profile header (see the suite line). Control arm of
// the source set: a header-triggered run is the one case the old hardcoded
// `source: Header` got right by accident, so this probe passing proves nothing
// on its own — it is here to show the other three differ from it for a reason
// other than the plumbing being broken for everybody.
//
// profiler/test_source_index reads the resulting run back off index.json.

$t = new TestCase('source_header', 'profiler');

$t->assertTrue('header trigger activated profiling', OxPHP\Profile\is_active());

function _src_header_leaf(int $n): int { return $n + 1; }
$t->assertSame('work ran under the profiler', _src_header_leaf(1), 2);

$t->done();
