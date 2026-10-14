<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// PHP 8.5 accepts a static closure in a parameter's default value. The
// compiler keeps such a closure in the default's constant expression, not
// among the closures the task's body declares, and evaluating the default
// creates it. The task is refused before its first statement all the same,
// so the counter stays at zero — whether the closure is the whole default or
// sits inside an array. A default whose closure does only built-in work is let
// through and runs.
$t = new TestCase('test_task_default_value_closure_calls_function', 'asyncdecl');

if (PHP_VERSION_ID < 80500) {
    $t->meta('php', 'PHP 8.4 has no closures in constant expressions');
    $t->assertTrue('nothing to check before PHP 8.5', true);
    $t->done();
}

$makeTasks = require __DIR__ . '/fixture_default_value_closure.php';
foreach (['the closure itself' => 0, 'a closure in an array' => 1] as $shape => $i) {
    $ran = new \OxPHP\Shared\Counter();
    [$ok, $result] = asyncdecl_settle(oxphp_async($makeTasks[$i]($ran)));

    $t->assertFalse("$shape: the task fails", $ok);
    $t->assertSame("$shape: with an Error naming the function and its call site", $result,
        'Async task failed: [Error] Call to undefined function asyncdecl_default_helper() at '
        . __DIR__ . '/fixture_default_value_closure.php:' . ASYNCDECL_DEFAULT_CALL_LINES[$i]
        . ASYNCDECL_TAIL);
    $t->assertSame("$shape: the task did not start", $ran->get(), 0);
}

$ran = new \OxPHP\Shared\Counter();
[$ok, $result] = asyncdecl_settle(oxphp_async($makeTasks[2]($ran)));
$t->assertTrue('built-in only: the task returns', $ok);
$t->assertSame('built-in only: it returns what the default closure did', $result, 5);
$t->assertSame('built-in only: the task ran', $ran->get(), 1);

$t->done();
