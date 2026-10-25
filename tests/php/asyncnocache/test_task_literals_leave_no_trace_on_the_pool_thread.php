<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_literals_leave_no_trace_on_the_pool_thread', 'asyncnocache');

// Without OPcache the literals of a task's code — an array written in the
// source, the strings inside it, a parameter default — live in the memory of
// the request that compiled it, and every thread that runs the code shares
// them. A thread that copies such a literal must not count the copy on the
// literal, and must not list the literal as a possible garbage root: both are
// writes into another thread's memory, and a root left in the task thread's
// buffer points into memory that the end of the request frees.
//
// Each task counts the roots its own thread gains around a copy of a literal.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$run = static fn (\Closure $task, mixed ...$args): mixed => oxphp_async_await(oxphp_async($task, ...$args));

$t->assertSame('a copy of an array literal leaves no root', $run(static function (): int {
    $roots = gc_status()['roots'];
    $a = [1, 2];
    unset($a);
    return gc_status()['roots'] - $roots;
}), 0);

// An array nested in a literal, and the strings inside it, are shared too.
[$added, $keyDump, $valueDump, $returnedDump] = $run(static function (): array {
    $roots = gc_status()['roots'];
    $a = ['literal-key' => ['literal-value', 3]];
    $inner = $a['literal-key'];
    unset($a, $inner);
    $added = gc_status()['roots'] - $roots;

    $b = ['dump-key' => ['dump-value', 3]];
    $key = array_key_first($b);
    $value = $b['dump-key'][0];
    ob_start();
    debug_zval_dump($key);
    $keyDump = ob_get_clean();
    ob_start();
    debug_zval_dump($value);
    $valueDump = ob_get_clean();
    // trim() returns the string it was given, counted unless the string itself
    // is flagged interned: the copy above only shows how the element is held.
    ob_start();
    debug_zval_dump(trim($value));
    return [$added, $keyDump, $valueDump, ob_get_clean()];
});
$t->assertSame('a copy of a nested array literal leaves no root', $added, 0);
$t->assertSame('the key of an array literal is not counted', $keyDump, "string(8) \"dump-key\" interned\n");
$t->assertSame('a string in an array literal is not counted', $valueDump, "string(10) \"dump-value\" interned\n");
$t->assertSame('a function returning a string of an array literal does not count it', $returnedDump, "string(10) \"dump-value\" interned\n");

// A literal nobody counts must still be copied before it is written to,
// also after a copy the HTTP thread held from before the dispatch is gone.
$cow = static function (bool $append): array {
    $w = [1, 2];
    if ($append) {
        $w[] = 3;
    }
    return $w;
};
$held = $cow(false);
$t->assertSame('the first task writes to its copy', $run($cow, true), [1, 2, 3]);
unset($held);
$t->assertSame('the second task writes to a fresh copy', $run($cow, true), [1, 2, 3]);
$t->assertSame('the literal itself is unchanged', $cow(false), [1, 2]);

// A comparison of a literal that runs the first dispatch of its code, through
// the __toString() of the value it is compared with.
$compared = static function (): array {
    return ['x'];
};
$stringable = new class ($compared, $run) {
    public function __construct(private \Closure $code, private \Closure $run)
    {
    }

    public function __toString(): string
    {
        ($this->run)($this->code);
        return 'x';
    }
};
$compared() == [$stringable];
try {
    $equal = $compared() == ['x'];
} catch (\Error $e) {
    $equal = $e->getMessage();
}
$t->assertSame('a literal compared while its code is dispatched compares again', $equal, true);

$t->assertSame('a literal of a closure the task creates leaves no root', $run(static function (): int {
    $inner = static function (): int {
        $roots = gc_status()['roots'];
        $a = [3, 4];
        unset($a);
        return gc_status()['roots'] - $roots;
    };
    return $inner();
}), 0);

// The constant is unknown when the file compiles, so the default stays an
// expression, evaluated by the task's thread from the literal in it. The
// constants a task defines stay on its thread after this request ends, so their
// names are built there rather than written in the code.
$t->assertSame('a parameter default leaves no root', $run(static function (): int {
    $withDefault = static function (array $a = OXPHP_LITERALS_TEST_ZERO ?: [5, 6]): int {
        $roots = gc_status()['roots'];
        unset($a);
        return gc_status()['roots'] - $roots;
    };
    defined('OXPHP_LITERALS_TEST_ZERO') || define(strtoupper('oxphp_literals_test_zero'), 0);
    return $withDefault();
}), 0);

// A constant a task defines outlives this request on the task's thread, so it
// must hold copies of the literals in its value, not the literals themselves.
$t->assertSame('a constant a task defines from an array literal holds its own copy', $run(static function (): string {
    defined('OXPHP_LITERALS_TEST_LIST') || define(strtoupper('oxphp_literals_test_list'), [13, 14]);
    ob_start();
    debug_zval_dump(OXPHP_LITERALS_TEST_LIST);
    return strtok(ob_get_clean(), "\n");
}), 'array(2) packed refcount(2){');
$t->assertSame('a constant a task defines from an array holding a copy of a literal holds its own copy', $run(static function (): string {
    $inner = [15, 16];
    defined('OXPHP_LITERALS_TEST_NESTED') || define(strtoupper('oxphp_literals_test_nested'), [$inner]);
    ob_start();
    debug_zval_dump(OXPHP_LITERALS_TEST_NESTED[0]);
    return strtok(ob_get_clean(), "\n");
}), 'array(2) packed refcount(2){');
$t->assertSame('a constant a task defines from a built array keeps its objects', $run(static function (): bool {
    defined('OXPHP_LITERALS_TEST_OBJECTS') || define(strtoupper('oxphp_literals_test_objects'), [new \stdClass()]);
    return OXPHP_LITERALS_TEST_OBJECTS[0] instanceof \stdClass;
}), true);

// Reflection copies attribute arguments out of the code they are written in.
$t->assertSame('an attribute argument leaves no root', $run(static function (): int {
    $tagged = static function (#[OxphpLiteralsTestTag([11, 12])] int $x): void {};
    $arguments = (new \ReflectionFunction($tagged))->getParameters()[0]->getAttributes()[0]->getArguments();
    $roots = gc_status()['roots'];
    unset($arguments);
    return gc_status()['roots'] - $roots;
}), 0);

// A closure in a constant expression, PHP 8.5 on, has literals of its own and is
// not among the closures its function declares. eval() keeps this file valid
// on 8.4.
if (PHP_VERSION_ID >= 80500) {
    $t->assertSame('a literal of a closure in a parameter default leaves no root', $run(eval(<<<'PHP'
        return static function (\Closure $inner = static function (): int {
            $roots = gc_status()['roots'];
            $a = [9, 10];
            unset($a);
            return gc_status()['roots'] - $roots;
        }): int {
            return $inner();
        };
        PHP)), 0);
}

// Last: the HTTP thread frees the code of this closure while a task's thread
// may still list its literal. Freeing it must leave this thread's buffer of
// roots as it was.
$f = eval('return static function (): int { $a = [7, 8]; unset($a); return 1; };');
$t->assertSame('the eval\'d task returns', $run($f), 1);
gc_collect_cycles();
$roots = gc_status()['roots'];
unset($f);
$t->assertSame('freeing the task\'s code leaves the roots of this thread as they were', gc_status()['roots'], $roots);

$t->done();
