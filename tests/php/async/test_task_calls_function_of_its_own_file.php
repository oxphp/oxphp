<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function async_own_file_helper(int $base = 0): int
{
    return $base + 5;
}

// A task calling a function of the script that dispatched it. The async pool's
// thread never ran this script: the task fails with an Error instead of taking
// the process down.
$t = new TestCase('test_task_calls_function_of_its_own_file', 'async');

$line = __LINE__ + 1;
$task = static fn (): int => async_own_file_helper();
try {
    $result = oxphp_async_await(oxphp_async($task));
} catch (\OxPHP\Async\AsyncException $e) {
    $result = $e->getMessage();
}

$t->assertSame('the task fails with an Error naming the function and its call site', $result,
    'Async task failed: [Error] Call to undefined function async_own_file_helper() at ' . __FILE__ . ":$line"
    . ': it belongs to a script this PHP thread has not loaded; only built-in functions and classes '
    . 'and those loaded by opcache.preload exist on every PHP thread, so the code was not run');

$t->done();
