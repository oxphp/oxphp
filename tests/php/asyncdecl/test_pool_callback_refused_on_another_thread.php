<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// On a thread other than the one that created the pool, a factory or $destroy
// runs with copies of its closure's variables, made there. A variable holding
// what is not copied, an object other than an OxPHP\Shared one, a closure
// among them, or a resource, is refused there, and so is a closure that uses
// $this, an object of the request that created the pool, and a method called
// on an object other than an OxPHP\Shared one, built-in or not: before the
// callback runs, as acquire() refuses one that reaches code the thread has not
// loaded. On the thread that created the pool the same callbacks run.
$t = new TestCase('test_pool_callback_refused_on_another_thread', 'asyncdecl');

function asyncdecl_refused_call_helper(int $base = 0): int
{
    return $base + 1;
}

$lib = __DIR__ . '/lib.php';

$obj = new \stdClass();
$line = __LINE__ + 1;
$capturesObject = static fn (): object => (object) ['v' => $obj::class];

$list = ['o' => new \stdClass()];
$listLine = __LINE__ + 1;
$capturesArrayOfObject = static fn (): object => (object) ['v' => \count($list)];

$fh = fopen('php://memory', 'r');
$fhLine = __LINE__ + 1;
$capturesResource = static fn (): object => (object) ['v' => get_resource_type($fh)];

$staticLine = __LINE__ + 1;
$staticObject = static function (): object {
    static $o = null;
    $o ??= new \stdClass();
    return (object) ['v' => 1];
};
$staticObject();

$mode = \Random\IntervalBoundary::ClosedClosed;
$modeLine = __LINE__ + 1;
$capturesEnumCase = static fn (): object => (object) ['v' => $mode->name];

$cyclic = [];
$cyclic['self'] = &$cyclic;
$cyclicLine = __LINE__ + 1;
$capturesCyclic = static fn (): object => (object) ['v' => \count($cyclic)];

$maker = new AsyncDeclPreMaker();
[$thisPool, $thisLine] = $maker->thisPool();
[$nestedPool, $nestedLine] = $maker->nestedThisPool();

$queue = new \SplQueue();
$queue->enqueue(new \stdClass());
$queue->enqueue(new \stdClass());
$reflection = new \ReflectionFunction(static fn (): object => new \stdClass());

$cases = [
    'a captured object' => [
        new \OxPHP\Shared\Pool(factory: $capturesObject, destroy: null, maxSize: 1),
        asyncdecl_not_copied('obj', 'an object of class stdClass', __FILE__, $line),
    ],
    'an object in a captured array' => [
        new \OxPHP\Shared\Pool(factory: $capturesArrayOfObject, destroy: null, maxSize: 1),
        asyncdecl_not_copied('list', 'an object of class stdClass', __FILE__, $listLine),
    ],
    'a captured resource' => [
        new \OxPHP\Shared\Pool(factory: $capturesResource, destroy: null, maxSize: 1),
        asyncdecl_not_copied('fh', 'a resource', __FILE__, $fhLine),
    ],
    'a static set to an object' => [
        new \OxPHP\Shared\Pool(factory: $staticObject, destroy: null, maxSize: 1),
        asyncdecl_not_copied('o', 'an object of class stdClass', __FILE__, $staticLine),
    ],
    'a captured enum case' => [
        new \OxPHP\Shared\Pool(factory: $capturesEnumCase, destroy: null, maxSize: 1),
        asyncdecl_not_copied('mode', 'an object of class Random\\IntervalBoundary', __FILE__, $modeLine),
    ],
    'a captured array that holds itself' => [
        new \OxPHP\Shared\Pool(factory: $capturesCyclic, destroy: null, maxSize: 1),
        asyncdecl_not_copied('cyclic', 'an array that holds itself', __FILE__, $cyclicLine),
    ],
    '$this in the factory' => [
        $thisPool,
        asyncdecl_uses_this("The closure at $lib:$thisLine"),
    ],
    '$this in a closure the factory declares' => [
        $nestedPool,
        asyncdecl_uses_this("The closure at $lib:$nestedLine"),
    ],
    '[$object, \'method\'] whose method uses $this' => [
        new \OxPHP\Shared\Pool(factory: [$maker, 'make'], destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreMaker::make()'),
    ],
    '$object->method(...) whose method uses $this' => [
        new \OxPHP\Shared\Pool(factory: $maker->make(...), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreMaker::make()'),
    ],
    'a __call() that uses $this' => [
        new \OxPHP\Shared\Pool(factory: [new AsyncDeclPreMagicMaker(), 'make'], destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreMagicMaker::__call()'),
    ],
    '$object->method(...) that only __call() answers' => [
        new \OxPHP\Shared\Pool(factory: (new AsyncDeclPreMagicMaker())->make(...), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreMagicMaker::__call()'),
    ],
    // The closure has the name of a method the class has, which __call()
    // answers outside it.
    '$object->method(...) that __call() answers for a private method' => [
        new \OxPHP\Shared\Pool(factory: (new AsyncDeclPreHiddenMaker())->make(...), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreHiddenMaker::__call()'),
    ],
    'Closure::fromCallable() of a private method __call() answers' => [
        new \OxPHP\Shared\Pool(factory: \Closure::fromCallable([new AsyncDeclPreHiddenMaker(), 'make']), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreHiddenMaker::__call()'),
    ],
    '$object->method(...) that __call() answers for a protected built-in method' => [
        new \OxPHP\Shared\Pool(factory: (new AsyncDeclPreHeapMaker())->compare(...), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreHeapMaker::__call()'),
    ],
    'an __invoke() that uses $this' => [
        new \OxPHP\Shared\Pool(factory: new AsyncDeclPreInvokableMaker(), destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreInvokableMaker::__invoke()'),
    ],
    // Bound to an object it does not use: the closure runs without it.
    'a factory with a $this it does not use' => [
        $maker->plainPool(),
        ['returned', \stdClass::class],
    ],
    // A method, though, has its object.
    '[$object, \'method\'] whose method does not use $this' => [
        new \OxPHP\Shared\Pool(factory: [$maker, 'plain'], destroy: null, maxSize: 1),
        asyncdecl_on_object('Method AsyncDeclPreMaker::plain()'),
    ],
    // A closure without $this calls a method that is not static as a static one.
    'parent:: on a method that is not static' => [
        (new AsyncDeclPreChildMaker())->parentPool(),
        [\Error::class, 'Non-static method AsyncDeclPreMaker::make() cannot be called statically'],
    ],
    // A built-in method works on the object it is called on.
    '[$queue, \'dequeue\']' => [
        new \OxPHP\Shared\Pool(factory: [$queue, 'dequeue'], destroy: null, maxSize: 1),
        asyncdecl_on_object('Built-in method SplQueue::dequeue()'),
    ],
    '$queue->dequeue(...)' => [
        new \OxPHP\Shared\Pool(factory: $queue->dequeue(...), destroy: null, maxSize: 1),
        asyncdecl_on_object('Built-in method SplQueue::dequeue()'),
    ],
    // The method runs a closure of this request.
    '[$reflection, \'invoke\']' => [
        new \OxPHP\Shared\Pool(factory: [$reflection, 'invoke'], destroy: null, maxSize: 1),
        asyncdecl_on_object('Built-in method ReflectionFunction::invoke()'),
    ],
    'a built-in method of an OxPHP\\Shared object' => [
        new \OxPHP\Shared\Pool(factory: [new \OxPHP\Shared\Channel(1), 'tryRecv'], destroy: null, maxSize: 1),
        ['returned', \OxPHP\Shared\Channel\RecvResult::class],
    ],
];

foreach ($cases as $what => [$pool, $expected]) {
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool), $expected);
    $h = $pool->acquire();
    $t->assertTrue("$what: on the worker thread the factory runs", \is_object($h->get()));
    $h->release();
    $pool->evict();
}

// A $destroy that captured an object is skipped in a task, and the slot is
// still dropped.
$ran = new \OxPHP\Shared\Counter(0);
$pool = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static function (object $r) use ($ran, $obj): void {
        $ran->add(1);
    },
    maxSize: 1,
);
[$ok, $evicted] = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $pool->acquire()->release();
    return $pool->evict();
}));
$t->assertTrue('$destroy: the task returns', $ok);
$t->assertSame('$destroy: evict() in the task dropped the slot', $evicted, 1);
$t->assertSame('$destroy: it did not run in the task', $ran->get(), 0);
$pool->acquire()->release();
$t->assertSame('$destroy: evict() on the worker thread dropped the slot', $pool->evict(), 1);
$t->assertSame('$destroy: it ran on the worker thread', $ran->get(), 1);

// A built-in method as $destroy is skipped in a task too: one that runs a
// closure of this request with the slot as its $this, and one that keeps the
// slot in an object of this request. Closure::call() binds a closure only to
// an object of a class written in PHP.
$ran = new \OxPHP\Shared\Counter(0);
$kept = new \SplQueue();
$destroys = [
    '[$closure, \'call\']' => [
        [function () use ($ran): void {
            $ran->add(asyncdecl_refused_call_helper());
        }, 'call'],
        static fn (): int => $ran->get(),
    ],
    '[$queue, \'push\']' => [[$kept, 'push'], static fn (): int => \count($kept)],
];
foreach ($destroys as $what => [$destroy, $runs]) {
    $pool = new \OxPHP\Shared\Pool(
        factory: static fn (): object => new AsyncDeclPreNamed(),
        destroy: $destroy,
        maxSize: 1,
    );
    [$ok, $evicted] = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
        $pool->acquire()->release();
        return $pool->evict();
    }));
    $t->assertTrue("$what as \$destroy: the task returns", $ok);
    $t->assertSame("$what as \$destroy: evict() in the task dropped the slot", $evicted, 1);
    $t->assertSame("$what as \$destroy: it did not run in the task", $runs(), 0);
    $pool->acquire()->release();
    $t->assertSame("$what as \$destroy: evict() on the worker thread dropped the slot", $pool->evict(), 1);
    $t->assertSame("$what as \$destroy: it ran on the worker thread", $runs(), 1);
}

fclose($fh);

$t->done();
