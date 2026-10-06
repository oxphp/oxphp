<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Leaves 24 MiB behind in the worker's persistent state, past the 16 MiB
// ceiling. The ceiling is checked after the request, so this one is answered
// normally and the worker recycles on its way to the next.
//
// $sharedState is the worker entry's static store, reachable here because an
// included file runs in the includer's scope. The string is computed, so it
// is allocated on the request heap that the ceiling measures.

$test = new TestCase('recycle_grow', 'recycle');

$sharedState['recycle_ballast'] = str_repeat('x', 24 * 1024 * 1024);

$worker = OxPHP\Server\Worker::current();
$test->assertGreaterThan('the worker is now past its ceiling', memory_get_usage(), $worker->maxMemoryBytes());

$test->done();
