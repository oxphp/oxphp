<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Activated by the OXPROF cookie (see the suite line), spelled the way the
// documentation tells an operator to set it from the browser: the whole cookie
// name, with no plugin prefix. The trigger reads it off the `Cookie` header
// rather than through the per-plugin cookie namespace, which strips a
// `__oxp_profiler_` prefix and so would only ever see `__oxp_profiler_OXPROF`.
// A probe sending that namespaced spelling instead would pass while the one
// name a user can discover activates nothing.
//
// profiler/test_source_index reads the resulting run back off index.json and
// asserts source == "Cookie".

$t = new TestCase('source_cookie', 'profiler');

$t->assertTrue('cookie trigger activated profiling', OxPHP\Profile\is_active());

function _src_cookie_leaf(int $n): int { return $n + 1; }
$t->assertSame('work ran under the profiler', _src_cookie_leaf(1), 2);

$t->done();
