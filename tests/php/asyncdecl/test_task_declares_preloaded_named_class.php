<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task declaring a named class that a preloaded file compiled: its
// definition is on every thread, so the declaration binds it.
$t = new TestCase('test_task_declares_preloaded_named_class', 'asyncdecl');

[$ok, $result] = asyncdecl_settle(oxphp_async(asyncdecl_pre_declaring_task()));

$t->assertSame('the task returns', [$ok, $result], [true, 14]);

$t->done();
