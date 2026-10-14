<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_unreached_helper(int $base = 0): int
{
    return $base + 31;
}

// The whole body is checked before the task runs, a branch it never takes
// included: a task that only names a function of this file in such a branch
// is refused too, every time.
$t = new TestCase('test_task_unreached_call_is_refused', 'asyncdecl');

$ran = new \OxPHP\Shared\Counter();
$never = false;
$line = __LINE__ + 4;
$task = static function () use ($ran, $never): int {
    $ran->add();
    if ($never) {
        return asyncdecl_unreached_helper();
    }
    return 1;
};
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertSame('with an Error naming the function and its call site', $result,
    'Async task failed: [Error] Call to undefined function asyncdecl_unreached_helper() at '
    . __FILE__ . ":$line" . ASYNCDECL_TAIL);
$t->assertSame('the task did not start', $ran->get(), 0);

$t->done();
