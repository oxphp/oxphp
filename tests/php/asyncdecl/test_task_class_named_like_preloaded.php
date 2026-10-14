<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task declaring a class under a name a preloaded class already holds. That
// is a redeclaration, which the engine reports itself on any thread.
$t = new TestCase('test_task_class_named_like_preloaded', 'asyncdecl');

$task = static function (): int {
    class AsyncDeclPreNamed
    {
    }
    return 0;
};
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertFalse('the task fails', $ok);
$t->assertContains('with the engine\'s own redeclaration error', $result,
    '[Error] Cannot redeclare class AsyncDeclPreNamed (previously declared in /var/www/html/public/tests/asyncdecl/lib.php:');

$t->done();
