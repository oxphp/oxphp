<?php

declare(strict_types=1);

// Loaded by opcache.preload, so all of it exists on every PHP thread.

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_pre_helper(int $base = 0): int
{
    return $base + 11;
}

final class AsyncDeclPreNamed
{
    public int $x = 12;
}

/** @return array<string, \Closure> */
function asyncdecl_pre_tasks(): array
{
    return [
        // A call to a function of this same file.
        'call' => static fn (): int => asyncdecl_pre_helper(),
        // An anonymous class, compiled with this file.
        'anon' => static fn (): int => (new class { public int $x = 13; })->x,
        'named' => static fn (): int => (new AsyncDeclPreNamed())->x,
    ];
}

// A named class declared inside a closure: run once, on one thread.
function asyncdecl_pre_declaring_task(): \Closure
{
    return static function (): int {
        class AsyncDeclPreDeclared
        {
            public int $x = 14;
        }
        return (new AsyncDeclPreDeclared())->x;
    };
}

// Callbacks of a Shared\Pool that reach $this in different ways. Each method
// returning a pool returns the line its factory starts on with it.
class AsyncDeclPreMaker
{
    public int $v = 21;

    public function make(): object
    {
        return (object) ['v' => $this->v];
    }

    public function plain(): object
    {
        return (object) ['v' => 22];
    }

    /** @return array{\OxPHP\Shared\Pool, int} */
    public function thisPool(): array
    {
        return [new \OxPHP\Shared\Pool(factory: fn (): object => $this->make(), destroy: null, maxSize: 1), __LINE__];
    }

    /** Only the closure the factory declares uses $this. @return array{\OxPHP\Shared\Pool, int} */
    public function nestedThisPool(): array
    {
        return [new \OxPHP\Shared\Pool(factory: fn (): object => (fn (): object => $this->make())(), destroy: null, maxSize: 1), __LINE__];
    }

    public function plainPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(factory: fn (): object => new \stdClass(), destroy: null, maxSize: 1);
    }
}

final class AsyncDeclPreChildMaker extends AsyncDeclPreMaker
{
    public function parentPool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(factory: fn (): object => parent::make(), destroy: null, maxSize: 1);
    }
}

final class AsyncDeclPreMagicMaker
{
    public int $v = 23;

    public function __call(string $name, array $args): object
    {
        return (object) ['v' => $this->v];
    }
}

/** __call() answers make() outside the class: make() is private. */
final class AsyncDeclPreHiddenMaker
{
    public int $v = 25;

    private function make(): object
    {
        return new \stdClass();
    }

    public function __call(string $name, array $args): object
    {
        return (object) ['v' => $this->v];
    }
}

/** __call() answers compare(), a protected built-in method, outside the class. */
final class AsyncDeclPreHeapMaker extends \SplMinHeap
{
    public int $v = 26;

    public function __call(string $name, array $args): object
    {
        return (object) ['v' => $this->v];
    }
}

final class AsyncDeclPreInvokableMaker
{
    public int $v = 24;

    public function __invoke(): object
    {
        return (object) ['v' => $this->v];
    }
}
