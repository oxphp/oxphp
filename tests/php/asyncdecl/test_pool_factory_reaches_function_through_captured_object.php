<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_captured_object_helper(int $base = 0): int
{
    return $base + 1;
}

// Factories that call nothing of this file themselves, only a method of an
// object they captured, which calls a function of this file. However the
// object was captured, a task's thread gets no copy of it, and the factory is
// refused for that. That thread does not look at what it captured: the
// request that created the pool can change a variable captured by reference
// while that thread would read it. acquire() in a task throws instead of
// taking the process down.
$t = new TestCase('test_pool_factory_reaches_function_through_captured_object', 'asyncdecl');

final class AsyncDeclCapturedMaker
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_captured_object_helper()];
    }
}

class AsyncDeclCapturedBaseMaker
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_captured_object_helper()];
    }
}

final class AsyncDeclCapturedChildMaker extends AsyncDeclCapturedBaseMaker
{
}


$maker = new AsyncDeclCapturedMaker();
$holds = 'an object of class ' . AsyncDeclCapturedMaker::class;

$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $maker->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'a captured object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('maker', $holds, __FILE__, $line),
);

$deps = ['maker' => $maker];
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $deps['maker']->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an object in a captured array',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('deps', $holds, __FILE__, $line),
);

$byRef = new AsyncDeclCapturedMaker();
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use (&$byRef): object {
        return $byRef->make();
    },
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an object captured by reference',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('byRef', $holds, __FILE__, $line),
);

$cyclic = [];
$cyclic['self'] = &$cyclic;
$cyclic['maker'] = $maker;
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $cyclic['maker']->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an array that holds itself, then the object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('cyclic', 'an array that holds itself', __FILE__, $line),
);
// The thread that created the pool looks at what the factory captured, the
// array once.
$h = $pool->acquire();
$t->assertSame('an array that holds itself: the factory runs on the worker thread', $h->get()->v, 1);
$h->release();

$suit = AsyncDeclPreSuit::Hearts;
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use ($suit, $maker): object {
        $o = $maker->make();
        $o->suit = $suit;
        return $o;
    },
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an enum case, then the object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('suit', 'an object of class ' . AsyncDeclPreSuit::class, __FILE__, $line),
);

$line = __LINE__ + 1;
$invoked = static fn (): object => $maker->make();
$pool = new \OxPHP\Shared\Pool(factory: [$invoked, '__invoke'], destroy: null, maxSize: 1);
$t->assertSame(
    'a closure that captured the object, as [$closure, \'__invoke\']',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('maker', $holds, __FILE__, $line),
);

// A closure bound to that closure as $this is refused for using $this.
$line = __LINE__ + 1;
$bound = \Closure::bind(fn (): object => ($this)(), $invoked, null);
$pool = new \OxPHP\Shared\Pool(factory: $bound, destroy: null, maxSize: 1);
$t->assertSame(
    'a closure whose $this is a closure that captured the object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_uses_this('The closure at ' . __FILE__ . ":$line"),
);

$child = new AsyncDeclCapturedChildMaker();
$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $child->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an object whose class inherits the method',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_not_copied('child', 'an object of class ' . AsyncDeclCapturedChildMaker::class, __FILE__, $line),
);

$h = $pool->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

$t->done();
