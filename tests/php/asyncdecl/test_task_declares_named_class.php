<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task whose body declares a named class. The declaration binds a class
// compiled with this file, and the task's thread never ran it.
$t = new TestCase('test_task_declares_named_class', 'asyncdecl');

$line = __LINE__ + 2;
$task = static function (): int {
    class AsyncDeclInner
    {
        public int $x = 10;
    }
    return (new AsyncDeclInner())->x;
};
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with an Error naming the class and where it is declared', $result,
    "Async task failed: [Error] Cannot declare class asyncdeclinner at " . __FILE__ . ":$line"
    . ASYNCDECL_TAIL);

$t->done();
