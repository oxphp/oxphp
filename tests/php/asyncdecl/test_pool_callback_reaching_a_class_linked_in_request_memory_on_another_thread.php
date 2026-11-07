<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Counter;
use OxPHP\Shared\Pool;

// An enum opcache.preload did not load, and a class whose parent, interface or
// trait is not in OPcache's shared memory, are linked by each request that loads
// them, into a copy that keeps the `static`s and run-time caches of the methods
// it declares, and its static properties, in that request's memory whichever
// thread reads them. On a thread other than the one that created the pool:
// - a method such a class declares with a `static` is refused;
// - one with none runs with a run-time cache of its own for the call;
// - a callback reaching such a class through self::, static::, parent::,
//   forward_static_call() or __callStatic(), or running eval() or include,
//   whose code takes the class of the frame that runs it, is refused, whichever
//   function the call runs first.
$t = new TestCase('test_pool_callback_reaching_a_class_linked_in_request_memory_on_another_thread', 'asyncdecl');

/** The `v` of a slot acquire() returns on this thread. */
function asyncdecl_linked_here(Pool $pool): mixed
{
    $h = $pool->acquire();
    $v = $h->get()->v;
    $h->release();
    $pool->evict();
    return $v;
}

enum AsyncDeclLinkedSuit
{
    case Hearts;

    public static function counted(): object
    {
        static $n = 0;
        return (object) ['v' => ++$n];
    }

    public static function plain(): object
    {
        return (object) ['v' => 9];
    }
}

asyncdecl_load_linked_classes();

$linked = static fn (string $class): array => [\Error::class, "Class $class" . ASYNCDECL_LINKED_TAIL];
$static = static fn (string $method): array => [\Error::class, "Method $method()" . ASYNCDECL_STATIC_TAIL];

// The `static` the request set: refused there, and not changed by the task.
AsyncDeclLinkedMaker::counted();
AsyncDeclLinkedSuit::counted();
foreach ([AsyncDeclLinkedMaker::class, AsyncDeclLinkedSuit::class] as $class) {
    $pool = new Pool(factory: [$class, 'counted'], destroy: null, maxSize: 1);
    $t->assertSame("$class::counted, by name: refused in a task", asyncdecl_acquire_in_task($pool, 'v'), $static("$class::counted"));
    $t->assertSame("$class::counted, by name: the task did not count", $class::counted()->v, 2);
    $t->assertSame("$class::counted, by name: runs on this thread", asyncdecl_linked_here($pool), 3);
    unset($pool);
}

// A `static` no call has set yet: the task leaves no table of its thread's in
// the method, which this thread would go on using.
$pool = new Pool(factory: [AsyncDeclLinkedMaker::class, 'neverCalledBefore'], destroy: null, maxSize: 1);
$t->assertSame(
    'a static never set: refused in a task',
    asyncdecl_acquire_in_task($pool, 'v'),
    $static('AsyncDeclLinkedMaker::neverCalledBefore'),
);
$t->assertSame('a static never set: this thread counts from its start', AsyncDeclLinkedMaker::neverCalledBefore()->v, 1);
unset($pool);

// With no `static`, by name and as a closure, a method runs in a task.
$runs = [
    'a method with no static, by name' => [[AsyncDeclLinkedMaker::class, 'plain'], 1],
    'a method with no static, as a closure' => [AsyncDeclLinkedMaker::plain(...), 1],
    'a method of an enum with no static, by name' => [[AsyncDeclLinkedSuit::class, 'plain'], 9],
    'a method of an enum with no static, as a closure' => [AsyncDeclLinkedSuit::plain(...), 9],
    // static:: read as a name only, with the class the call names.
    'static::class, by name' => [[AsyncDeclLinkedMaker::class, 'calledClass'], AsyncDeclLinkedMaker::class],
    'static::class, as a closure' => [AsyncDeclLinkedMaker::calledClass(...), AsyncDeclLinkedMaker::class],
    // A class OPcache keeps linked in shared memory is not this request's.
    'self::helper() of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'viaSelf'], 1],
    'self::$n of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'viaSelfProperty'], 1],
    'static::LIMIT of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'viaStaticConstant'], 6],
    'new static of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'viaNewStatic'], 8],
    '__callStatic() of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'nope'], 1],
    '__callStatic() of a class in shared memory, as a closure' => [AsyncDeclLinkedInSharedMemory::nope(...), 1],
    'forward_static_call() of a preloaded class, by name' => [[AsyncDeclPreForwarder::class, 'forwarded'], 0],
    // eval() has the classes self and static stand for checked, as self:: does;
    // with neither, nothing is.
    'eval() of a class in shared memory, by name' => [[AsyncDeclLinkedInSharedMemory::class, 'viaEval'], 1],
    'eval() in a closure with no class' => [static fn (): object => (object) ['v' => eval('return 2;')], 2],
    // Inherited from a parent in shared memory, the method is the parent's, with
    // what each thread has of its own, whichever class links it.
    'a static method inherited from a parent in shared memory, by name'
        => [[AsyncDeclLinkedShmChild::class, 'inheritedCounted'], 'inherited'],
    'a static method inherited from a parent in shared memory, as a closure'
        => [AsyncDeclLinkedShmChild::inheritedCounted(...), 'inherited'],
];
foreach ($runs as $what => [$factory, $v]) {
    $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: runs in a task", asyncdecl_acquire_in_task($pool, 'v'), ['returned', $v]);
    unset($pool);
}

// Run through its own run-time cache, a method would leave there what the
// other thread found, and each thread would find it: this thread's class on the
// task's, which never loaded it. A class of this script is looked up again by
// the task whichever of them ran the method first.
$notFound = [\Error::class, 'Class "AsyncDeclLinkedTarget" not found'];
$pool = new Pool(factory: [AsyncDeclLinkedMaker::class, 'namesATaskFirst'], destroy: null, maxSize: 1);
$t->assertSame('a class it names, by name: not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
$t->assertSame('a class it names, by name: found on this thread', AsyncDeclLinkedMaker::namesATaskFirst()->v, 16);
$t->assertSame('a class it names, by name: still not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
unset($pool);

$t->assertSame('run here first, by name: found on this thread', AsyncDeclLinkedMaker::namesAThreadFirst()->v, 16);
$pool = new Pool(factory: [AsyncDeclLinkedMaker::class, 'namesAThreadFirst'], destroy: null, maxSize: 1);
$t->assertSame('run here first, by name: not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
unset($pool);

$pool = new Pool(factory: AsyncDeclLinkedMaker::namesATaskFirstAsAClosure(...), destroy: null, maxSize: 1);
$t->assertSame('a class it names, as a closure: not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
$t->assertSame('a class it names, as a closure: found on this thread', AsyncDeclLinkedMaker::namesATaskFirstAsAClosure()->v, 16);
$t->assertSame('a class it names, as a closure: still not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
unset($pool);

$t->assertSame('run here first, as a closure: found on this thread', AsyncDeclLinkedMaker::namesAThreadFirstAsAClosure()->v, 16);
$pool = new Pool(factory: AsyncDeclLinkedMaker::namesAThreadFirstAsAClosure(...), destroy: null, maxSize: 1);
$t->assertSame('run here first, as a closure: not found in a task', asyncdecl_acquire_in_task($pool), $notFound);
unset($pool);

// A callback that reaches the class is refused whichever way it does, and runs
// on the thread that created the pool.
$reaching = [
    'self::helper()' => [[AsyncDeclLinkedMaker::class, 'viaSelf'], AsyncDeclLinkedMaker::class, 3],
    'self::helper(), as a closure' => [AsyncDeclLinkedMaker::viaSelf(...), AsyncDeclLinkedMaker::class, 3],
    'parent::make() of a parent in request memory' => [[AsyncDeclLinkedMaker::class, 'viaParent'], AsyncDeclLinkedMaker::class, 4],
    'new static' => [[AsyncDeclLinkedMaker::class, 'viaNewStatic'], AsyncDeclLinkedMaker::class, 7],
    'self::$n' => [[AsyncDeclLinkedMaker::class, 'viaSelfProperty'], AsyncDeclLinkedMaker::class, 1],
    // A task that ran it would count from this thread's 1, and this thread on
    // from the task's.
    'self::$n in code eval() runs' => [[AsyncDeclLinkedMaker::class, 'viaEval'], AsyncDeclLinkedMaker::class, 2],
    'self::$n in a file it includes' => [[AsyncDeclLinkedMaker::class, 'viaInclude'], AsyncDeclLinkedMaker::class, 3],
    'static::LIMIT' => [[AsyncDeclLinkedMaker::class, 'viaStaticConstant'], AsyncDeclLinkedMaker::class, 5],
    'a closure the class declares, self::helper()' => [AsyncDeclLinkedMaker::declaredClosure(), AsyncDeclLinkedMaker::class, 3],
    '__callStatic(), by name' => [[AsyncDeclLinkedMagic::class, 'nope'], AsyncDeclLinkedMagic::class, 1],
    '__callStatic(), as a closure' => [AsyncDeclLinkedMagic::nope(...), AsyncDeclLinkedMagic::class, 2],
    'static::helper() from a parent in shared memory, by name'
        => [[AsyncDeclLinkedShmChild::class, 'viaStatic'], AsyncDeclLinkedShmChild::class, 1],
    'static::helper() from a parent in shared memory, as a closure'
        => [AsyncDeclLinkedShmChild::viaStatic(...), AsyncDeclLinkedShmChild::class, 2],
    'static::helper() in code eval() runs in a parent in shared memory, by name'
        => [[AsyncDeclLinkedShmChild::class, 'viaEvalStatic'], AsyncDeclLinkedShmChild::class, 3],
    "the parent's __callStatic(), by name" => [[AsyncDeclLinkedShmChild::class, 'nope'], AsyncDeclLinkedShmChild::class, 2],
    "the parent's __callStatic(), as a closure" => [AsyncDeclLinkedShmChild::nope(...), AsyncDeclLinkedShmChild::class, 2],
    'self::helper() of an anonymous class'
        => [[asyncdecl_linked_anonymous_class(), 'viaSelf'], 'AsyncDeclLinkedEvalParent@anonymous', 3],
    'new static of a class the type of a method keeps out of shared memory'
        => [[AsyncDeclLinkedByVariance::class, 'viaNewStatic'], AsyncDeclLinkedByVariance::class, 11],
];
foreach ($reaching as $what => [$factory, $class, $here]) {
    $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: refused in a task", asyncdecl_acquire_in_task($pool, 'v'), $linked($class));
    $t->assertSame("$what: runs on this thread", asyncdecl_linked_here($pool), $here);
    unset($pool);
}

// Called from a method of a parent, forward_static_call() passes on the class
// that method was called through, and static:: there reaches this request's
// copy with nothing in the method to show it. The task does not count here.
AsyncDeclLinkedForwarding::helper();
$forwarding = [
    'forward_static_call(), by name' => [[AsyncDeclLinkedForwarding::class, 'forwarded'], 2],
    'forward_static_call(), as a closure' => [AsyncDeclLinkedForwarding::forwarded(...), 3],
    'forward_static_call_array(), by name' => [[AsyncDeclLinkedForwarding::class, 'forwardedArray'], 4],
    'forward_static_call(), named in another case' => [[AsyncDeclLinkedForwarding::class, 'forwardedInAnotherCase'], 5],
    'forward_static_call(), named with a leading backslash'
        => [[AsyncDeclLinkedForwarding::class, 'forwardedWithALeadingBackslash'], 6],
    'forward_static_call() in a method the class declares, with no static'
        => [[AsyncDeclLinkedForwarding::class, 'forwardsItself'], 7],
];
foreach ($forwarding as $what => [$factory, $here]) {
    $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: refused in a task", asyncdecl_acquire_in_task($pool, 'v'), $linked(AsyncDeclLinkedForwarding::class));
    $t->assertSame("$what: runs on this thread", asyncdecl_linked_here($pool), $here);
    unset($pool);
}

// $destroy goes the same way as a factory: with a `static` it is skipped, and
// evict() drops the slot all the same; with none it runs, with the slot.
$pool = new Pool(factory: 'date_create', destroy: [AsyncDeclLinkedMaker::class, 'destroyCounted'], maxSize: 1);
[$ok, $evicted] = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $pool->acquire()->release();
    return $pool->evict();
}));
$t->assertSame('$destroy with a static: evict() in a task drops the slot', [$ok, $evicted], [true, 1]);
$t->assertSame('$destroy with a static: refused in a task', AsyncDeclLinkedMaker::destroyCounted(null), 0);
unset($pool);

$destroyed = new Counter(0);
$pool = new Pool(
    factory: static fn (): object => (object) ['destroyed' => $destroyed],
    destroy: [AsyncDeclLinkedMaker::class, 'destroyPlain'],
    maxSize: 1,
);
[$ok, $evicted] = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $pool->acquire()->release();
    return $pool->evict();
}));
$t->assertSame('$destroy with no static: evict() in a task drops the slot', [$ok, $evicted], [true, 1]);
$t->assertSame('$destroy with no static: runs in a task', $destroyed->get(), 1);
unset($pool);

// Closure::getCurrent() tells a closure written in PHP from a function: the
// method still is one, as its name calls it on this thread.
if (PHP_VERSION_ID >= 80500) {
    $notClosure = ['returned', 'Current function is not a closure'];
    foreach (['by name' => [AsyncDeclLinkedMaker::class, 'currentClosure'], 'as a closure' => AsyncDeclLinkedMaker::currentClosure(...)] as $form => $factory) {
        $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
        $t->assertSame("Closure::getCurrent(), $form: throws in a task", asyncdecl_acquire_in_task($pool, 'v'), $notClosure);
        unset($pool);
    }
}

$t->done();
