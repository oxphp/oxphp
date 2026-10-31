<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;

// A closure made from a function or a static method, as f(...) and
// Closure::fromCallable() make one, uses that function's `static` variables
// as the request that made it has them, in its memory. A task's acquire()
// that ran such a factory changed them with memory of the task's thread, and
// the request, changing them in turn, took the process down ("zend_mm_heap
// corrupted"). On a thread other than the one that created the pool it runs as
// the function itself, as the function's name would, with that thread's own
// `static`s; on the thread that created the pool it shares them with the
// function as before.
$t = new TestCase('test_pool_callback_made_from_a_function_runs_as_it_on_another_thread', 'asyncdecl');

/**
 * acquire() a slot of $pool twice in a task, evicting it after each: the
 * $property of each slot, or [false, the task's failure].
 */
function asyncdecl_made_from_twice_in_task(Pool $pool, string $property): array
{
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool, $property): array {
        $seen = [];
        for ($i = 0; $i < 2; $i++) {
            $h = $pool->acquire();
            $seen[] = $h->get()->$property;
            $h->release();
            $pool->evict();
        }
        return $seen;
    }));
    return $ok ? $result : [false, $result];
}

function asyncdecl_made_from_named(): object
{
    static $n = 0;
    return (object) ['n' => ++$n];
}

function asyncdecl_made_from_never_called(): object
{
    static $n = 0;
    return (object) ['n' => ++$n];
}

function asyncdecl_made_from_appended(): object
{
    static $log = '';
    $log .= str_repeat('x', 3);
    return (object) ['log' => $log];
}

final class AsyncDeclMadeFromCachedClass
{
    public int $v = 32;
}

function asyncdecl_made_from_cached_class(): object
{
    return new AsyncDeclMadeFromCachedClass();
}

enum AsyncDeclMadeFromSuit
{
    case Hearts;

    public static function counted(): object
    {
        static $n = 0;
        return (object) ['n' => ++$n];
    }
}

/** acquire() a slot of $pool on this thread: its $property. */
function asyncdecl_made_from_here(Pool $pool, string $property): mixed
{
    $h = $pool->acquire();
    $value = $h->get()->$property;
    $h->release();
    return $value;
}

// A class whose parent is not in OPcache's shared memory is linked by the
// request that includes it, and its methods keep their `static`s in that
// request's memory whichever thread calls them: one that has some is refused
// on another thread, one that has none runs there.
eval(<<<'PHP'
    class AsyncDeclMadeFromEvalParent
    {
    }
    PHP);
require __DIR__ . '/fixture_child_of_eval_parent.php';

$pool = new Pool(factory: AsyncDeclMadeFromLateChild::counted(...), destroy: null, maxSize: 1);
$t->assertSame(
    'a method of a class this request linked, with a static: refused in a task',
    asyncdecl_acquire_in_task($pool),
    [\Error::class, 'Method AsyncDeclMadeFromLateChild::counted()' . ASYNCDECL_STATIC_TAIL],
);
$t->assertSame(
    'a method of a class this request linked, with a static: runs on the thread that created the pool',
    asyncdecl_made_from_here($pool, 'n'),
    1,
);
unset($pool);

$pool = new Pool(factory: AsyncDeclMadeFromLateChild::plain(...), destroy: null, maxSize: 1);
$t->assertSame(
    'a method of a class this request linked, with no static: runs in a task',
    asyncdecl_acquire_in_task($pool),
    ['returned', \stdClass::class],
);
unset($pool);

// Run there as the method itself, its first call would leave this thread's
// method a run-time cache in that thread's memory, and the class this thread
// then found there would be found by the task too. Closure::fromCallable()
// gives the closure a run-time cache of its own while the method has none;
// AsyncDeclMadeFromLateChild::made(...) gives the method one first and
// shares it.
$pool = new Pool(
    factory: Closure::fromCallable([AsyncDeclMadeFromLateChild::class, 'made']),
    destroy: null,
    maxSize: 1,
);
$late_target_missing = [\Error::class, 'Class "AsyncDeclMadeFromLateTarget" not found'];
$t->assertSame(
    'a method of a class this request linked, naming a class: not found in a task',
    asyncdecl_acquire_in_task($pool),
    $late_target_missing,
);
$t->assertSame(
    'a method of a class this request linked, naming a class: found on this thread',
    AsyncDeclMadeFromLateChild::made()->v,
    16,
);
$t->assertSame(
    'a method of a class this request linked, naming a class: still not found in a task',
    asyncdecl_acquire_in_task($pool),
    $late_target_missing,
);
unset($pool);

// So is every enum opcache.preload did not load.
$pool = new Pool(factory: AsyncDeclMadeFromSuit::counted(...), destroy: null, maxSize: 1);
$t->assertSame(
    'a method of an enum, with a static: refused in a task',
    asyncdecl_acquire_in_task($pool),
    [\Error::class, 'Method AsyncDeclMadeFromSuit::counted()' . ASYNCDECL_STATIC_TAIL],
);
unset($pool);

// The function's name: each thread has `static`s of its own.
asyncdecl_made_from_named();
$pool = new Pool(factory: 'asyncdecl_made_from_named', destroy: null, maxSize: 1);
$t->assertSame('by name: the task counts from its own', asyncdecl_made_from_twice_in_task($pool, 'n'), [1, 2]);
unset($pool);
$t->assertSame('by name: this thread counts on from its own', asyncdecl_made_from_named()->n, 2);

// A static method of a preloaded class, through a class static:: stands for.
AsyncDeclPreChildCounter::next();
$pool = new Pool(factory: AsyncDeclPreChildCounter::next(...), destroy: null, maxSize: 1);
$t->assertSame('a static method: the task counts from its own', asyncdecl_made_from_twice_in_task($pool, 'n'), [1, 2]);
$t->assertSame(
    'a static method: static:: is the class the closure was made through',
    asyncdecl_made_from_twice_in_task($pool, 'class'),
    [AsyncDeclPreChildCounter::class, AsyncDeclPreChildCounter::class],
);
$t->assertSame("a static method: on this thread the pool's closure counts on from the method's", asyncdecl_made_from_here($pool, 'n'), 2);
unset($pool);
$t->assertSame('a static method: the method counts on from the closure', AsyncDeclPreCounter::next()->n, 3);

// A function this thread never called: what the task sets is not this
// thread's.
$pool = new Pool(factory: Closure::fromCallable('asyncdecl_made_from_never_called'), destroy: null, maxSize: 1);
$t->assertSame('never called here: the task counts from its own', asyncdecl_made_from_twice_in_task($pool, 'n'), [1, 2]);
unset($pool);
$t->assertSame('never called here: this thread counts from its own', asyncdecl_made_from_never_called()->n, 1);

// The run-time cache is the thread's own too: a class of this script that
// this thread found is looked up again on the task's thread, which has none.
$pool = new Pool(factory: asyncdecl_made_from_cached_class(...), destroy: null, maxSize: 1);
$t->assertSame('a class it names: found on this thread', asyncdecl_made_from_here($pool, 'v'), 32);
$pool->evict();
$t->assertSame(
    'a class it names: not found in a task',
    asyncdecl_acquire_in_task($pool),
    [\Error::class, 'Class "AsyncDeclMadeFromCachedClass" not found'],
);
unset($pool);

// A string this thread put in a `static`, which the task appended to in this
// thread's table, and this thread then freed.
asyncdecl_made_from_appended();
$pool = new Pool(factory: asyncdecl_made_from_appended(...), destroy: null, maxSize: 1);
$t->assertSame('a string: the task appends to its own', asyncdecl_made_from_twice_in_task($pool, 'log'), ['xxx', 'xxxxxx']);
$t->assertSame("a string: on this thread the pool's closure appends to the function's", asyncdecl_made_from_here($pool, 'log'), 'xxxxxx');
unset($pool);
$t->assertSame('a string: the function appends to the closure\'s', asyncdecl_made_from_appended()->log, 'xxxxxxxxx');

$t->done();
