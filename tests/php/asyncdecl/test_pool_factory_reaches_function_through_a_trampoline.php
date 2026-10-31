<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_trampoline_reach_helper(int $base = 0): int
{
    return $base + 1;
}

// Factories that reach __call(), __callStatic() or a closure's __invoke, each
// of which calls a function of this file, directly or through a closure made
// from the method name. The function such a callable resolves to has no body of
// its own; the class, or the closure, behind it is checked.
// A __callStatic() that calls nothing of this file runs in a task:
// test_pool_trampoline_factory_in_task.
$t = new TestCase('test_pool_factory_reaches_function_through_a_trampoline', 'asyncdecl');

$callLine = __LINE__ + 5;
final class AsyncDeclReachingCall
{
    public function __call(string $name, array $args): object
    {
        return (object) ['v' => asyncdecl_trampoline_reach_helper()];
    }
}

$callStaticLine = __LINE__ + 5;
final class AsyncDeclReachingCallStatic
{
    public static function __callStatic(string $name, array $args): object
    {
        return (object) ['v' => asyncdecl_trampoline_reach_helper()];
    }
}

class AsyncDeclReachingCallParent
{
    public function __call(string $name, array $args): object
    {
        return $this->build();
    }
}

$childLine = __LINE__ + 5;
final class AsyncDeclReachingCallChild extends AsyncDeclReachingCallParent
{
    public function build(): object
    {
        return (object) ['v' => asyncdecl_trampoline_reach_helper()];
    }
}

$pool = new \OxPHP\Shared\Pool(factory: [new AsyncDeclReachingCall(), 'make'], destroy: null, maxSize: 1);
$t->assertSame(
    '__call()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callLine),
);

$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclReachingCallStatic::class, 'make'], destroy: null, maxSize: 1);
$t->assertSame(
    '__callStatic()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callStaticLine),
);

// A closure made from a method only __call() or __callStatic() answers is a
// built-in function, which calls the magic method.
$pool = new \OxPHP\Shared\Pool(factory: (new AsyncDeclReachingCall())->make(...), destroy: null, maxSize: 1);
$t->assertSame(
    '$object->make(...) through __call()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callLine),
);

$pool = new \OxPHP\Shared\Pool(factory: AsyncDeclReachingCallStatic::make(...), destroy: null, maxSize: 1);
$t->assertSame(
    'Class::make(...) through __callStatic()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callStaticLine),
);

$pool = new \OxPHP\Shared\Pool(
    factory: \Closure::fromCallable([new AsyncDeclReachingCall(), 'make']),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'Closure::fromCallable() through __call()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callLine),
);

$made = (new AsyncDeclReachingCall())->make(...);
$pool = new \OxPHP\Shared\Pool(factory: static fn (): object => $made(), destroy: null, maxSize: 1);
$t->assertSame(
    'a captured $object->make(...)',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callLine),
);

// The parent's __call() reaches a method of the child through $this.
$pool = new \OxPHP\Shared\Pool(factory: (new AsyncDeclReachingCallChild())->make(...), destroy: null, maxSize: 1);
$t->assertSame(
    '$object->make(...) through a parent\'s __call()',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $childLine),
);

// self:: in a closure bound to an object of another class: the closure it
// makes is called on that class, and __callStatic() is not one of its methods.
$made = \Closure::bind(fn (): \Closure => self::make(...), new \stdClass(), AsyncDeclReachingCallStatic::class)();
$pool = new \OxPHP\Shared\Pool(factory: $made, destroy: null, maxSize: 1);
$t->assertSame(
    'self::make(...) through __callStatic(), called on another class',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $callStaticLine),
);

$invokeLine = __LINE__ + 1;
$invoked = static fn (): object => (object) ['v' => asyncdecl_trampoline_reach_helper()];
$pool = new \OxPHP\Shared\Pool(factory: [$invoked, '__invoke'], destroy: null, maxSize: 1);
$t->assertSame(
    '[$closure, \'__invoke\']',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_trampoline_reach_helper', __FILE__, $invokeLine),
);

$h = $pool->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

$t->done();
