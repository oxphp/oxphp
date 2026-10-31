<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_destructor_helper(object $o): void
{
}

// A factory handing out clones of an object it captured, whose class has a
// __destruct() calling a function of this file. That destructor would run on
// the task's thread when its evict() drops the slot. The task's thread gets no
// copy of the object, and the factory is refused for that, so no slot of it is
// made there.
$t = new TestCase('test_pool_slot_destructor_reaches_function_in_task', 'asyncdecl');

final class AsyncDeclDestructedSlot
{
    public function __destruct()
    {
        asyncdecl_destructor_helper($this);
    }
}

$proto = new AsyncDeclDestructedSlot();
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => clone $proto,
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'acquire() in the task throws',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('proto', 'an object of class ' . AsyncDeclDestructedSlot::class, __FILE__, $line),
);

$pool->acquire()->release();
$t->assertSame('on this thread: evict() dropped the slot', [$pool->evict(), $pool->stats()->size()], [1, 0]);

$t->done();
