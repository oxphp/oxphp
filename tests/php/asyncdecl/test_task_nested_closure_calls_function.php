<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_nested_helper(int $base = 0): int
{
    return $base + 21;
}

// The call is in a closure the task creates, not in the task's own body. The
// task is refused before its first statement, so the counter stays at zero.
$t = new TestCase('test_task_nested_closure_calls_function', 'asyncdecl');

$ran = new \OxPHP\Shared\Counter();
$line = __LINE__ + 3;
$task = static function () use ($ran): int {
    $ran->add();
    $inner = static fn (): int => asyncdecl_nested_helper();
    return $inner();
};
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with an Error naming the function and its call site', $result,
    'Async task failed: [Error] Call to undefined function asyncdecl_nested_helper() at '
    . __FILE__ . ":$line" . ASYNCDECL_TAIL);
$t->assertSame('the task did not start', $ran->get(), 0);

$t->done();
