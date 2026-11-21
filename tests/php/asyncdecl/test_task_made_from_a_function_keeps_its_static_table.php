<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A closure made from a function (f(...)) uses the `static` table of the
// function rather than a copy of its own, and a task made from one runs with
// the table its thread built from the copy it was sent. The task reaches its
// `static` only after it waited, and its thread must not have freed that table
// by then.
$t = new TestCase('test_task_made_from_a_function_keeps_its_static_table', 'asyncdecl');

$t->assertSame(
    'a closure, which copies the table: the static starts from 0',
    asyncdecl_settle(oxphp_async(static fn (): array => asyncdecl_pre_static_after_a_wait(true))),
    [true, [1, 1, 512]],
);
$t->assertSame(
    'a closure made from the function: the static starts from 0',
    asyncdecl_settle(oxphp_async(asyncdecl_pre_static_after_a_wait(...), true)),
    [true, [1, 1, 512]],
);
$t->assertSame(
    'the same again: the table is the task\'s own',
    asyncdecl_settle(oxphp_async(asyncdecl_pre_static_after_a_wait(...), true)),
    [true, [1, 1, 512]],
);

$t->done();
