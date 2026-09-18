<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Activated by the ?__oxprof= query parameter (see the suite line).
// profiler/test_source_index reads the resulting run back off index.json and
// asserts source == "Query".

$t = new TestCase('source_query', 'profiler');

$t->assertTrue('query trigger activated profiling', OxPHP\Profile\is_active());

function _src_query_leaf(int $n): int { return $n + 1; }
$t->assertSame('work ran under the profiler', _src_query_leaf(1), 2);

$t->done();
