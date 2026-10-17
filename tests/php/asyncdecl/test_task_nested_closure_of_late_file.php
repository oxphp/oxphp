<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';
require_once __DIR__ . '/bulk.php';

// A task from a file this worker thread included after a file of thousands of
// functions: OPcache numbered the per-thread slots of the closures the task
// creates past the slots the task's thread had. The closures run, and the
// server stays up.
$t = new TestCase('test_task_nested_closure_of_late_file', 'asyncdecl');

$bulk = asyncdecl_bulk('closures');
$fixture = __DIR__ . '/fixture_late_closures.php';
$task = require $fixture;
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertTrue('the generated file is cached', opcache_is_script_cached($bulk));
$t->assertTrue('the task\'s file is cached', opcache_is_script_cached($fixture));
$t->assertTrue('the task returns', $ok);
$t->assertSame('what its closures computed', $result, [[2, 4, 6], 11, 1, 2]);

$t->done();
