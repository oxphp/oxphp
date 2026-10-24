<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_captured_closure_helper(int $base = 0): int
{
    return $base + 1;
}

// A factory calling a closure it captured, declared outside the factory, which
// calls a function of this file. The captured closure is checked with the
// factory. A closure that captured itself is checked once, and the check goes
// on to what the factory captured after it.
$t = new TestCase('test_pool_factory_reaches_function_through_captured_closure', 'asyncdecl');

$line = __LINE__ + 1;
$make = static fn (): object => (object) ['v' => asyncdecl_captured_closure_helper()];
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'a captured closure',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_captured_closure_helper', __FILE__, $line),
);

$h = $pool->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

$makerLine = __LINE__ + 5;
final class AsyncDeclCapturedClosureMaker
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_captured_closure_helper()];
    }
}

$f = static function (int $n) use (&$f): int {
    return $n > 0 ? $f($n - 1) : 0;
};
$maker = new AsyncDeclCapturedClosureMaker();
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use ($f, $maker): object {
        $o = $maker->make();
        $o->v += $f(2);
        return $o;
    },
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'a closure that captured itself, then an object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_captured_closure_helper', __FILE__, $makerLine),
);

// Nothing is missing on this thread, so all of it is checked before the
// factory runs.
$h = $pool->acquire();
$t->assertSame('on the worker thread, the same factory runs', $h->get()->v, 1);
$h->release();

$t->done();
