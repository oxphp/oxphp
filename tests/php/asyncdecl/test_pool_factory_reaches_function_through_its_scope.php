<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// Factories that reach a method or a property hook through self::, static::,
// new self or new static: it is found from the closure's class, not by a name
// this thread's tables must hold. Those classes are checked with the factory
// when it names one of them that way, and only then.
$t = new TestCase('test_pool_factory_reaches_function_through_its_scope', 'asyncdecl');

$makeLine = __LINE__ + 5;
class AsyncDeclScopeMaker
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_scope_helper()];
    }

    public static function selfPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => self::make(),
            destroy: null,
            maxSize: 1,
        );
    }

    public static function newStaticPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => (new static())->make(),
            destroy: null,
            maxSize: 1,
        );
    }

    public static function plainPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => new \stdClass(),
            destroy: null,
            maxSize: 1,
        );
    }
}

// static:: names the class the method was called on, which here is a subclass
// of the one the closure was declared in, and only the subclass's make() calls
// a function of this file.
class AsyncDeclStaticBase
{
    public static function make(): object
    {
        return new \stdClass();
    }

    public static function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => static::make(),
            destroy: null,
            maxSize: 1,
        );
    }
}

$childLine = __LINE__ + 5;
final class AsyncDeclStaticChild extends AsyncDeclStaticBase
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_scope_helper()];
    }
}

// A hook is not a method: PHP keeps it with its property. Before the hook that
// calls a function of this file, the class has a property without hooks and
// one with only a get hook, which calls nothing of this file.
$hookLine = __LINE__ + 11;
final class AsyncDeclScopeHookedMaker
{
    public int $plain = 0;

    public int $twice {
        get => $this->plain * 2;
    }

    public int $v {
        get {
            return asyncdecl_scope_helper();
        }
    }

    public static function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => (object) ['v' => (new self())->v],
            destroy: null,
            maxSize: 1,
        );
    }
}

$refused = asyncdecl_refused('asyncdecl_scope_helper', __FILE__, $makeLine);

$t->assertSame('self::', asyncdecl_acquire_in_task(AsyncDeclScopeMaker::selfPool()), $refused);
$t->assertSame('new static', asyncdecl_acquire_in_task(AsyncDeclScopeMaker::newStaticPool()), $refused);
$t->assertSame(
    'static::, called on a subclass',
    asyncdecl_acquire_in_task(AsyncDeclStaticChild::pool()),
    asyncdecl_refused('asyncdecl_scope_helper', __FILE__, $childLine),
);
$t->assertSame(
    'a property hook of an object of new self',
    asyncdecl_acquire_in_task(AsyncDeclScopeHookedMaker::pool()),
    asyncdecl_refused('asyncdecl_scope_helper', __FILE__, $hookLine),
);

// Bound to an object of an unrelated class, self:: still names the class the
// closure was bound to.
$bound = \Closure::bind(
    function (): object {
        return self::make();
    },
    new \stdClass(),
    AsyncDeclScopeMaker::class,
);
$pool = new \OxPHP\Shared\Pool(factory: $bound, destroy: null, maxSize: 1);
$t->assertSame('self:: of a closure bound to another class\'s object', asyncdecl_acquire_in_task($pool), $refused);

$t->assertSame(
    'a factory declared in the same class that names no class through it runs in the task',
    asyncdecl_acquire_in_task(AsyncDeclScopeMaker::plainPool()),
    ['returned', \stdClass::class],
);

$h = AsyncDeclScopeMaker::selfPool()->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

$t->done();
