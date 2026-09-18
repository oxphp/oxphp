<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Activated by the OXPROF cookie (see the suite line). The suite sends it under
// the plugin cookie prefix — `__oxp_profiler_OXPROF` — because that is the only
// spelling the plugin can currently see: cookies reach a plugin through a
// per-plugin prefixed namespace, and the bare name the profiler looks up is the
// name *after* that prefix is stripped. Sending the bare `OXPROF` activates
// nothing. That gap is tracked separately; this probe is about what happens
// once the cookie arm does fire, so it uses the spelling that gets there.
//
// profiler/test_source_index reads the resulting run back off index.json and
// asserts source == "Cookie".

$t = new TestCase('source_cookie', 'profiler');

$t->assertTrue('cookie trigger activated profiling', OxPHP\Profile\is_active());

function _src_cookie_leaf(int $n): int { return $n + 1; }
$t->assertSame('work ran under the profiler', _src_cookie_leaf(1), 2);

$t->done();
