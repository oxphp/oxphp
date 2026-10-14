<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task declaring an anonymous class of the worker's entry file. The class was
// compiled with that file, which the task's thread never ran.
$t = new TestCase('test_task_declares_anonymous_class', 'asyncdecl');

[$line, $task] = $fixtures['anon'];
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with an Error naming where the class is declared', $result,
    "Async task failed: [Error] Cannot declare anonymous class at /var/www/html/asyncdecl/worker.php:$line"
    . ASYNCDECL_TAIL);

$t->done();
