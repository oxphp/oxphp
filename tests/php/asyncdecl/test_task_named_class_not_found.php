<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A named class of the worker's entry file is looked up by name, with
// autoloading, and a failed lookup throws. That stays the engine's own Error.
$t = new TestCase('test_task_named_class_not_found', 'asyncdecl');

[$ok, $result] = asyncdecl_settle(oxphp_async($fixtures['named']));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with the engine\'s own Error', $result,
    'Async task failed: [Error] Class "AsyncDeclNamed" not found');

$t->done();
