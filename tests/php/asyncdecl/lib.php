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
