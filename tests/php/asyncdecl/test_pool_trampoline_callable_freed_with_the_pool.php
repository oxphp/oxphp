<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory naming a method its class does not have resolves to a function
// the engine builds for that one callable, to reach __call(). The pool keeps
// it for as long as it keeps the factory, and frees it along with the
// factory: before, every such pool left one behind until the request ended.
// The pool keeps a copy of that function, and makes one for every call it
// makes through it, which that call frees together with the name the copy
// gets; any other callable it keeps and calls as it is.
$t = new TestCase('test_pool_trampoline_callable_freed_with_the_pool', 'asyncdecl');

final class AsyncDeclTrampolineLeakMaker
{
    public function __call(string $name, array $args): object
    {
        return new \stdClass();
    }
}

/** How much memory $run() leaves behind, once a first run has warmed up. */
$grown = static function (callable $run): int {
    $run(20);
    $before = memory_get_usage();
    $run(200);
    return memory_get_usage() - $before;
};

$maker = new AsyncDeclTrampolineLeakMaker();
$t->assertLessThan(
    '200 pools with a factory reaching __call() come and go without leaving memory behind',
    $grown(static function (int $n) use ($maker): void {
        for ($i = 0; $i < $n; $i++) {
            $pool = new \OxPHP\Shared\Pool(factory: [$maker, 'make'], destroy: null, maxSize: 1);
            unset($pool);
        }
    }),
    4096,
);

$make = static fn (): object => new \stdClass();
$t->assertLessThan(
    '200 pools with a closure factory come and go without leaving memory behind',
    $grown(static function (int $n) use ($make): void {
        for ($i = 0; $i < $n; $i++) {
            $pool = new \OxPHP\Shared\Pool(factory: $make, destroy: null, maxSize: 1);
            unset($pool);
        }
    }),
    4096,
);

$pool = new \OxPHP\Shared\Pool(factory: $make, destroy: null, maxSize: 1);
$t->assertLessThan(
    'a closure factory run 200 times leaves no memory behind',
    $grown(static function (int $n) use ($pool): void {
        for ($i = 0; $i < $n; $i++) {
            $pool->acquire()->release();
            $pool->evict();
        }
    }),
    4096,
);

$pool = new \OxPHP\Shared\Pool(factory: [$maker, 'make'], destroy: null, maxSize: 1);
$t->assertLessThan(
    'a factory reaching __call() run 200 times leaves no memory behind',
    $grown(static function (int $n) use ($pool): void {
        for ($i = 0; $i < $n; $i++) {
            $pool->acquire()->release();
            $pool->evict();
        }
    }),
    4096,
);

$t->done();
