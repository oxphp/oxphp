<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory naming a method its class does not have runs __call() or
// __callStatic() through a function the engine frees once it has been called,
// and so does [$closure, '__invoke']. The pool resolves its factory once and
// runs it for every slot it mints, so the second acquire() ran a freed
// function: it took the whole process down, or failed with an unrelated
// error.
$t = new TestCase('test_pool_trampoline_factory_called_twice', 'asyncdecl');

final class AsyncDeclTrampolineMaker
{
    public function __call(string $name, array $args): object
    {
        $o = new \stdClass();
        $o->name = $name;
        return $o;
    }

    public static function __callStatic(string $name, array $args): object
    {
        $o = new \stdClass();
        $o->name = $name;
        return $o;
    }
}

$make = static function (): object {
    $o = new \stdClass();
    $o->name = 'make';
    return $o;
};

$factories = [
    '__call()' => [new AsyncDeclTrampolineMaker(), 'make'],
    '__callStatic()' => AsyncDeclTrampolineMaker::class . '::make',
    '__invoke' => [$make, '__invoke'],
];
foreach ($factories as $via => $factory) {
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 2);
    $a = $pool->acquire();
    $b = $pool->acquire();
    $t->assertSame("$via: the first slot", $a->get()->name, 'make');
    $t->assertSame("$via: the second slot", $b->get()->name, 'make');
    $t->assertTrue("$via: two slots", $a->get() !== $b->get());
    $a->release();
    $b->release();
    unset($a, $b, $pool);
}

$t->done();
