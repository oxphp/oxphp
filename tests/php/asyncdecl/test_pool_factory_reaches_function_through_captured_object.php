<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_captured_object_helper(int $base = 0): int
{
    return $base + 1;
}

// Factories that call nothing of this file themselves, only a method or a
// property hook of an object they captured, which calls a function of this
// file. The object's class is checked with the factory, however the object was
// captured: acquire() in a task throws instead of taking the process down.
$t = new TestCase('test_pool_factory_reaches_function_through_captured_object', 'asyncdecl');

$makerLine = __LINE__ + 5;
final class AsyncDeclCapturedMaker
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_captured_object_helper()];
    }
}

$inheritedLine = __LINE__ + 5;
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

// A hook is not a method: PHP keeps it with its property. Before the hook that
// calls a function of this file, the class has a property without hooks and
// one with only a get hook, which calls nothing of this file.
$hookLine = __LINE__ + 11;
final class AsyncDeclCapturedHookedMaker
{
    public int $plain = 0;

    public int $twice {
        get => $this->plain * 2;
    }

    public int $v {
        get {
            return asyncdecl_captured_object_helper();
        }
    }
}

$maker = new AsyncDeclCapturedMaker();
$refused = asyncdecl_refused('asyncdecl_captured_object_helper', __FILE__, $makerLine);

$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $maker->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame('a captured object', asyncdecl_acquire_in_task($pool), $refused);

$deps = ['maker' => $maker];
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $deps['maker']->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame('an object in a captured array', asyncdecl_acquire_in_task($pool), $refused);

$byRef = new AsyncDeclCapturedMaker();
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use (&$byRef): object {
        return $byRef->make();
    },
    destroy: null,
    maxSize: 1,
);
$t->assertSame('an object captured by reference', asyncdecl_acquire_in_task($pool), $refused);

// The array is checked once: the object after the reference to itself is reached.
$cyclic = [];
$cyclic['self'] = &$cyclic;
$cyclic['maker'] = $maker;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $cyclic['maker']->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame('an array that holds itself, then the object', asyncdecl_acquire_in_task($pool), $refused);

// cases() of an enum is a built-in function the enum declares itself. The
// enum is preloaded: one opcache.preload did not load is refused for itself.
$suit = AsyncDeclPreSuit::Hearts;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use ($suit, $maker): object {
        $o = $maker->make();
        $o->suit = $suit;
        return $o;
    },
    destroy: null,
    maxSize: 1,
);
$t->assertSame('an enum case, then the object', asyncdecl_acquire_in_task($pool), $refused);

$hooked = new AsyncDeclCapturedHookedMaker();
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => (object) ['v' => $hooked->v],
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'a property hook of a captured object',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_captured_object_helper', __FILE__, $hookLine),
);

$child = new AsyncDeclCapturedChildMaker();
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => $child->make(),
    destroy: null,
    maxSize: 1,
);
$t->assertSame(
    'an object whose class inherits the method',
    asyncdecl_acquire_in_task($pool),
    asyncdecl_refused('asyncdecl_captured_object_helper', __FILE__, $inheritedLine),
);

$h = $pool->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 1);
$h->release();

$t->done();
