<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_this_helper(int $base = 0): int
{
    return $base + 1;
}

// A factory declared in a method, calling a method of $this. $this is an
// object of a subclass of the class the closure was declared in, and only the
// subclass's make() calls a function of this file. $this's class is checked
// with the factory when the factory uses $this, and only then.
$t = new TestCase('test_pool_factory_reaches_function_through_this', 'asyncdecl');

class AsyncDeclThisBase
{
    public function make(): object
    {
        return new \stdClass();
    }

    public function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: fn (): object => $this->make(),
            destroy: null,
            maxSize: 1,
        );
    }

    // Only the closure the factory declares uses $this.
    public function nestedPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: fn (): object => (fn (): object => $this->make())(),
            destroy: null,
            maxSize: 1,
        );
    }

    public function plainPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: fn (): object => new \stdClass(),
            destroy: null,
            maxSize: 1,
        );
    }
}

$makeLine = __LINE__ + 5;
final class AsyncDeclThisChild extends AsyncDeclThisBase
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_this_helper()];
    }
}

$child = new AsyncDeclThisChild();

$refused = asyncdecl_refused('asyncdecl_this_helper', __FILE__, $makeLine);

$t->assertSame('$this->make()', asyncdecl_acquire_in_task($child->pool()), $refused);
$t->assertSame(
    '$this->make() in a closure the factory declares',
    asyncdecl_acquire_in_task($child->nestedPool()),
    $refused,
);
$t->assertSame(
    'a factory with the same $this that does not use it runs in the task',
    asyncdecl_acquire_in_task($child->plainPool()),
    ['returned', \stdClass::class],
);

$h = $child->pool()->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

// $this can be a closure: bound with no scope, it is called as the call it
// makes, not checked by its class, which is built in.
$innerLine = __LINE__ + 1;
$inner = fn (): object => (object) ['v' => asyncdecl_this_helper(1)];
$bound = new \OxPHP\Shared\Pool(
    factory: \Closure::bind(fn (): object => ($this)(), $inner, null),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    '($this)() with a closure bound as $this',
    asyncdecl_acquire_in_task($bound),
    asyncdecl_refused('asyncdecl_this_helper', __FILE__, $innerLine),
);
$h = $bound->acquire();
$t->assertSame('the bound closure calls $this on the worker thread', $h->get()->v, 2);
$h->release();

$t->done();
