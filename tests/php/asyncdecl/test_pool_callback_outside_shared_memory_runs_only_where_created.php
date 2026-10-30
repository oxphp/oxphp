<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Counter;
use OxPHP\Shared\Pool;

// Code eval() compiles stays in the memory of the request that ran it, as all
// code does without OPcache's shared memory, and its literals with it. A
// task's thread that ran such a callback counted those literals and listed
// them among its own possible garbage roots, and a slot it made held them
// after that request had ended. On a thread other than the one that created
// the pool such a callback is refused, whichever form of callable leads to the
// code; on the thread that created the pool it runs.
$t = new TestCase('test_pool_callback_outside_shared_memory_runs_only_where_created', 'asyncdecl');

// Each body copies an array literal into a variable, which a task's thread
// that ran it lists as a possible garbage root when the variable goes away.
eval(<<<'PHP'
    function asyncdecl_eval_make(): object
    {
        $a = ['ts' => 86400];
        return date_create('@' . $a['ts']);
    }

    final class AsyncDeclEvalMaker
    {
        public static function build(): object
        {
            $a = ['ts' => 86400];
            return date_create('@' . $a['ts']);
        }

        public function __invoke(): object
        {
            $a = ['ts' => 86400];
            return date_create('@' . $a['ts']);
        }

        public function __call(string $name, array $args): object
        {
            $a = ['ts' => 86400];
            return date_create('@' . $a['ts']);
        }

        public static function __callStatic(string $name, array $args): object
        {
            $a = ['ts' => 86400];
            return date_create('@' . $a['ts']);
        }
    }
    PHP);
$closure = eval(<<<'PHP'
    return static function (): object {
        $a = ['ts' => 86400];
        return date_create('@' . $a['ts']);
    };
    PHP);
$maker = new AsyncDeclEvalMaker();

/**
 * acquire() a slot of $pool in a task, and one of a pool of date_create():
 * [the roots the date_create() pool added to the task's thread, the roots
 * $pool's acquire() added, refused or not], or [false, the task's failure].
 *
 * @return array{mixed, mixed}
 */
function asyncdecl_outside_shm_roots(Pool $pool): array
{
    $builtin = new Pool(factory: 'date_create', destroy: null, maxSize: 1);
    gc_collect_cycles();
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool, $builtin): array {
        $pool->stats();
        $builtin->stats();
        $roots = gc_status()['roots'];
        $b = $builtin->acquire();
        $control = gc_status()['roots'] - $roots;
        $roots = gc_status()['roots'];
        try {
            $h = $pool->acquire();
        } catch (\Error) {
            $h = null;
        }
        $added = gc_status()['roots'] - $roots;
        $h?->release();
        $b->release();
        $pool->evict();
        $builtin->evict();
        return [$control, $added];
    }));
    return $ok ? $result : [false, $result];
}

$factories = [
    'a closure' => [$closure, $closure],
    'the name of a function' => ['asyncdecl_eval_make', 'Function asyncdecl_eval_make()'],
    'a closure made from a function' => [asyncdecl_eval_make(...), 'Function asyncdecl_eval_make()'],
    'a static method' => [[AsyncDeclEvalMaker::class, 'build'], 'Method AsyncDeclEvalMaker::build()'],
    'an object with __invoke()' => [$maker, 'Method AsyncDeclEvalMaker::__invoke()'],
    'a method only __call() answers' => [[$maker, 'make'], 'Method AsyncDeclEvalMaker::__call()'],
    'a closure made from a method only __call() answers'
        => [$maker->make(...), 'Method AsyncDeclEvalMaker::__call()'],
    'a static method only __callStatic() answers'
        => [[AsyncDeclEvalMaker::class, 'make'], 'Method AsyncDeclEvalMaker::__callStatic()'],
    'a closure made from a static method only __callStatic() answers'
        => [AsyncDeclEvalMaker::make(...), 'Method AsyncDeclEvalMaker::__callStatic()'],
    "[\$closure, '__invoke']" => [[$closure, '__invoke'], $closure],
];

foreach ($factories as $what => [$factory, $named]) {
    $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: refused in a task", asyncdecl_acquire_in_task($pool), asyncdecl_outside_shm($named));
    $h = $pool->acquire();
    $t->assertSame("$what: runs on the thread that created the pool", $h->get()->getTimestamp(), 86400);
    $h->release();
}

// What the refusal keeps off the task's thread: the literals of this request,
// which a callback that ran lists there on top of what a slot of date_create()
// does. Code no task has run: a thread does not list a value its buffer holds.
eval(<<<'PHP'
    function asyncdecl_eval_roots(): object
    {
        $a = ['ts' => 86400];
        return date_create('@' . $a['ts']);
    }
    PHP);
$fresh = eval(<<<'PHP'
    return static function (): object {
        $a = ['ts' => 86400];
        return date_create('@' . $a['ts']);
    };
    PHP);
foreach (['a closure' => $fresh, 'the name of a function' => 'asyncdecl_eval_roots'] as $what => $factory) {
    [$control, $added] = asyncdecl_outside_shm_roots(new Pool(factory: $factory, destroy: null, maxSize: 1));
    $t->assertTrue("$what: the roots task returns", $control !== false);
    if ($control !== false) {
        $t->assertLessThan("$what: lists no literal on the task's thread", $added - $control, 1);
    }
}

// The same code in OPcache's shared memory, and a built-in function, run there.
$cached = static function (): object {
    $a = ['ts' => 86400];
    return date_create('@' . $a['ts']);
};
$t->assertSame(
    'a closure in shared memory runs in a task',
    asyncdecl_acquire_in_task(new Pool(factory: $cached, destroy: null, maxSize: 1)),
    ['returned', \DateTime::class],
);
$t->assertSame(
    'a built-in function runs in a task',
    asyncdecl_acquire_in_task(new Pool(factory: 'date_create', destroy: null, maxSize: 1)),
    ['returned', \DateTime::class],
);

// from() of a backed enum is a built-in method of a class written in PHP that
// has a __callStatic(). A task's thread, whose tables do not hold the enum,
// refuses it for the enum, not for the code of __callStatic().
eval(<<<'PHP'
    enum AsyncDeclEvalSuit: string
    {
        case Hearts = 'h';

        public static function __callStatic(string $name, array $args): object
        {
            $a = ['ts' => 86400];
            return date_create('@' . $a['ts']);
        }
    }
    PHP);
$t->assertSame(
    'a built-in method of an enum with __callStatic() is refused for the enum',
    asyncdecl_acquire_in_task(new Pool(factory: [AsyncDeclEvalSuit::class, 'from'], destroy: null, maxSize: 1)),
    [\Error::class, 'Enum AsyncDeclEvalSuit' . ASYNCDECL_TAIL],
);

// A refused $destroy is skipped: evict() drops the slot all the same.
$destroyed = new Counter(0);
$destroy = eval(<<<'PHP'
    return static function (object $r) use ($destroyed): void {
        $a = ['n' => 1];
        $destroyed->add($a['n']);
    };
    PHP);
$pool = new Pool(factory: 'date_create', destroy: $destroy, maxSize: 1);
$result = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $pool->acquire()->release();
    return $pool->evict();
}));
$t->assertSame('$destroy: evict() in a task drops the slot', $result, [true, 1]);
$t->assertSame('$destroy: refused in a task', $destroyed->get(), 0);
$pool->acquire()->release();
$t->assertSame('$destroy: evict() on the thread that created the pool drops the slot', $pool->evict(), 1);
$t->assertSame('$destroy: runs on the thread that created the pool', $destroyed->get(), 1);

// Last: this thread frees the code of a callback a task was handed. Freeing
// it must leave this thread's buffer of roots as it was.
$factory = eval('return static function (): object { $a = [7, 8]; return date_create("@" . $a[0]); };');
$refused = asyncdecl_outside_shm($factory);
$pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
unset($factory);
$t->assertSame('the factory freed last: refused in a task', asyncdecl_acquire_in_task($pool), $refused);
gc_collect_cycles();
$roots = gc_status()['roots'];
unset($pool);
$t->assertSame('freeing the callback\'s code leaves the roots of this thread as they were', gc_status()['roots'], $roots);

$t->done();
