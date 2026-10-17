<?php

declare(strict_types=1);

// Included by test_pool_named_factory_created_after_task_started after a
// generated file of thousands of functions. The factory uses each of its
// per-thread slots: the runtime cache, the static variable, and the closure it
// creates.
function asyncdecl_late_factory(): object
{
    static $made = 0;
    $double = static fn (int $x): int => \intdiv($x * 4, 2);
    $o = new \stdClass();
    $o->made = ++$made;
    $o->doubled = \array_map($double, [1, 2, 3]);
    return $o;
}
