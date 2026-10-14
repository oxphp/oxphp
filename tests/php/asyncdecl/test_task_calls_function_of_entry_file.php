<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task calling a function of the worker's entry file. The call compiles to a
// direct call the engine never checks, and the task's thread never ran that
// file: the task must fail with an Error, not take the process down.
$t = new TestCase('test_task_calls_function_of_entry_file', 'asyncdecl');

[$line, $task] = $fixtures['call'];
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with an Error naming the function and its call site', $result,
    'Async task failed: [Error] Call to undefined function asyncdecl_helper() at '
    . "/var/www/html/asyncdecl/worker.php:$line" . ASYNCDECL_TAIL);
$t->assertSame('the function still works on the worker thread', $task(), 7);

$t->done();
