<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A function a task's thread enters is checked as it is entered: one calling a
// function the thread lacks does not start, its own try included, and the code
// calling it gets the Error. Each factory here reaches a method of this file only
// through forward_static_call() under names built at run time, so the check made
// before the callback starts has nothing to find: str_rot13() keeps the names out
// of the file's literals, and array_rand(), which OPcache's optimizer cannot
// evaluate, keeps them out of the compiled code as well. forward_static_call()
// passes this class on as the called class, and the preloaded method it calls
// reaches back into this class through static::.
$t = new TestCase('test_pool_callee_refused_as_it_is_entered', 'asyncdecl');

function asyncdecl_entry_helper(int $base = 0): int
{
    return $base + 1;
}

final class AsyncDeclEntryChild extends AsyncDeclPreForwarder
{
    public const HELPER_LINE = __LINE__ + 5;

    public static function helper(): object
    {
        try {
            return (object) ['v' => asyncdecl_entry_helper(1)];
        } catch (\Error) {
            AsyncDeclPreForwarder::$caught++;
            return (object) ['v' => asyncdecl_entry_helper(2)];
        }
    }

    public const NUMBERS_LINE = __LINE__ + 4;

    public static function numbers(): \Generator
    {
        yield asyncdecl_entry_helper(2);
    }

    // The preloaded method calls helper() from the VM.
    public static function viaCall(): object
    {
        $caught = AsyncDeclPreForwarder::$caught;
        try {
            return \str_rot13(\array_rand(['sbejneq_fgngvp_pnyy' => true]))(
                [AsyncDeclPreForwarder::class, \str_rot13(\array_rand(['gnetrg' => true]))],
            );
        } catch (\Error $e) {
            throw new \RuntimeException(\sprintf('caught %d: %s', AsyncDeclPreForwarder::$caught - $caught, $e->getMessage()));
        }
    }

    // The preloaded method has array_map() call helper().
    public static function viaNativeCall(): object
    {
        $caught = AsyncDeclPreForwarder::$caught;
        try {
            return \str_rot13(\array_rand(['sbejneq_fgngvp_pnyy' => true]))(
                [AsyncDeclPreForwarder::class, \str_rot13(\array_rand(['sbejneqrqAngviryl' => true]))],
            );
        } catch (\Error $e) {
            throw new \RuntimeException(\sprintf('caught %d: %s', AsyncDeclPreForwarder::$caught - $caught, $e->getMessage()));
        }
    }

    // The preloaded method calls numbers() and iterates the generator it returns.
    public static function viaGenerator(): object
    {
        $made = AsyncDeclPreForwarder::$made;
        try {
            return \str_rot13(\array_rand(['sbejneq_fgngvp_pnyy' => true]))(
                [AsyncDeclPreForwarder::class, \str_rot13(\array_rand(['vgrengrq' => true]))],
            );
        } catch (\Error $e) {
            throw new \RuntimeException(\sprintf('made %d: %s', AsyncDeclPreForwarder::$made - $made, $e->getMessage()));
        }
    }
}

[, $helperRefused] = asyncdecl_refused('asyncdecl_entry_helper', __FILE__, AsyncDeclEntryChild::HELPER_LINE);
[, $numbersRefused] = asyncdecl_refused('asyncdecl_entry_helper', __FILE__, AsyncDeclEntryChild::NUMBERS_LINE);
$cases = [
    'a method called from the VM' => ['viaCall', "caught 0: $helperRefused", 2],
    'a method called from native code' => ['viaNativeCall', "caught 0: $helperRefused", 2],
    // PHP calls no observer as a generator function is called, only as its
    // generator resumes: the generator is made, and its body does not start.
    'a generator' => ['viaGenerator', "made 1: $numbersRefused", 3],
];
foreach ($cases as $what => [$factory, $refused, $v]) {
    $pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclEntryChild::class, $factory], destroy: null, maxSize: 1);
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool, 'v'), [\RuntimeException::class, $refused]);
    $h = $pool->acquire();
    $t->assertSame("$what: on the worker thread the factory runs", $h->get()->v, $v);
    $h->release();
}

$t->done();
