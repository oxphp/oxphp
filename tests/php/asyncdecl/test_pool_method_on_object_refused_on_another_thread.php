<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// On a thread other than the one that created the pool, a factory or $destroy
// that is a method written in PHP, called on an object other than an
// OxPHP\Shared one, is refused even when its code never names $this: each of
// these reaches the object another way, and the property hook calls its
// parent's on it. On the thread that created the pool they run on the object.
$t = new TestCase('test_pool_method_on_object_refused_on_another_thread', 'asyncdecl');

$quiet = new AsyncDeclPreQuietMaker();
$magic = new AsyncDeclPreQuietMagicMaker();
$hook = (new \ReflectionProperty(AsyncDeclPreHookMaker::class, 'made'))
    ->getHook(\PropertyHookType::Get)
    ->getClosure(new AsyncDeclPreHookMaker());

$cases = [
    'self:: on a method that is not static' => [[$quiet, 'viaSelf'], 'AsyncDeclPreQuietMaker::viaSelf'],
    'a closure the method declares' => [[$quiet, 'viaClosure'], 'AsyncDeclPreQuietMaker::viaClosure'],
    'debug_backtrace()' => [[$quiet, 'viaBacktrace'], 'AsyncDeclPreQuietMaker::viaBacktrace'],
    '$object->method(...)' => [$quiet->viaSelf(...), 'AsyncDeclPreQuietMaker::viaSelf'],
    '[$object->method(...), \'__invoke\']' => [[$quiet->viaSelf(...), '__invoke'], 'AsyncDeclPreQuietMaker::viaSelf'],
    'an __invoke()' => [new AsyncDeclPreQuietInvokableMaker(), 'AsyncDeclPreQuietInvokableMaker::__invoke'],
    'a __call()' => [[$magic, 'make'], 'AsyncDeclPreQuietMagicMaker::__call'],
    '$object->method(...) that only __call() answers' => [$magic->make(...), 'AsyncDeclPreQuietMagicMaker::__call'],
    'a property hook calling its parent\'s' => [$hook, 'AsyncDeclPreHookMaker::$made::get'],
    // PHP calls it on the $this of the method the pool was created in.
    '[self::class, \'method\'] for a method that is not static' => [
        $quiet->classCallablePool(),
        'AsyncDeclPreQuietMaker::viaBacktrace',
    ],
];

foreach ($cases as $what => [$factory, $method]) {
    $pool = $factory instanceof \OxPHP\Shared\Pool
        ? $factory
        : new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool), asyncdecl_on_object("Method $method()"));
    $h = $pool->acquire();
    $t->assertSame("$what: on the worker thread the method has its object", $h->get()->v, true);
    $h->release();
    $pool->evict();
}

// A static method named through an object has none, and runs.
$pool = new \OxPHP\Shared\Pool(factory: [$quiet, 'made'], destroy: null, maxSize: 1);
$t->assertSame('[$object, \'static method\']: in a task', asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);

// A $destroy reaching its object through self:: is skipped in a task, and the
// slot is still dropped.
$ran = new \OxPHP\Shared\Counter(0);
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new AsyncDeclPreNamed(),
    destroy: [new AsyncDeclPreQuietMaker($ran), 'destroyViaSelf'],
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
$t->assertSame('$destroy: it reached its object on the worker thread', $ran->get(), 1);

$t->done();
