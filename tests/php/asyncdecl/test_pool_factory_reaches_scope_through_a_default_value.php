<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_default_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// A parameter's default value is evaluated with the class of the function it
// belongs to: new self() and new parent() there create an object of that
// class or of its parent, whose methods the call can then reach. Such a class
// is checked whole, as for new self() in the body. A constant read through
// self:: in a default value creates no object, and the class's methods are not
// checked for it.
$t = new TestCase('test_pool_factory_reaches_scope_through_a_default_value', 'asyncdecl');

$buildLine = __LINE__ + 5;
class AsyncDeclDefaultMaker
{
    public function build(): object
    {
        return (object) ['v' => asyncdecl_default_scope_helper()];
    }

    // Untyped: on PHP 8.5 a class type fails its check on a task's thread,
    // which does not hold the class, before the body runs.
    public static function make($maker = new self()): object
    {
        return $maker->build();
    }

    public static function constPool(): \OxPHP\Shared\Pool
    {
        // In a closure self:: is not resolved when the file is compiled, so
        // the constant is read when the default is evaluated.
        return new \OxPHP\Shared\Pool(
            factory: static fn (int $n = self::N): object => (object) ['v' => $n],
            destroy: null,
            maxSize: 1,
        );
    }

    public const N = 3;
}

final class AsyncDeclDefaultChild extends AsyncDeclDefaultMaker
{
    public static function makeParent($maker = new parent()): object
    {
        return $maker->build();
    }
}

// A constant's name written as an expression is evaluated with the default
// value: the object new self() creates there is thrown away at once.
$destructLine = __LINE__ + 5;
final class AsyncDeclDefaultNamedMaker
{
    public function __destruct()
    {
        asyncdecl_default_scope_helper();
    }

    public static function make($n = self::{new self() ? 'N' : 'N'}): object
    {
        return (object) ['v' => $n];
    }

    public const N = 1;
}

$refused = asyncdecl_refused('asyncdecl_default_scope_helper', __FILE__, $buildLine);
$selfPool = new \OxPHP\Shared\Pool(factory: [AsyncDeclDefaultMaker::class, 'make'], destroy: null, maxSize: 1);
$parentPool = new \OxPHP\Shared\Pool(factory: [AsyncDeclDefaultChild::class, 'makeParent'], destroy: null, maxSize: 1);
$namedPool = new \OxPHP\Shared\Pool(factory: [AsyncDeclDefaultNamedMaker::class, 'make'], destroy: null, maxSize: 1);

$t->assertSame('new self() as a default value', asyncdecl_acquire_in_task($selfPool), $refused);
$t->assertSame('new parent() as a default value', asyncdecl_acquire_in_task($parentPool), $refused);
$t->assertSame(
    'new self() in the name of a constant a default value reads',
    asyncdecl_acquire_in_task($namedPool),
    asyncdecl_refused('asyncdecl_default_scope_helper', __FILE__, $destructLine),
);
$t->assertSame(
    'self::N as a default value runs in the task',
    asyncdecl_acquire_in_task(AsyncDeclDefaultMaker::constPool()),
    ['returned', \stdClass::class],
);

foreach (['new self()' => $selfPool, 'new parent()' => $parentPool, 'self::{new self() ...}' => $namedPool] as $what => $pool) {
    $h = $pool->acquire();
    $t->assertSame("$what: the factory still works on the worker thread", $h->get()->v, 1);
    $h->release();
}

$t->done();
