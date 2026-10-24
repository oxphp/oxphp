<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory or $destroy that a task's acquire() or evict() runs is an object of
// this request, and this request frees it. Calling it must not leave it in the
// task thread's buffer of possible garbage roots: an entry there points into
// memory the task's thread does not own, and the collection that reads it
// takes the server down at a later request.
//
// Each task counts the roots its thread gains while the callback runs, and the
// roots it gains while a pool of the same shape whose callback is a built-in
// function runs: what the slot itself adds is the same in both.
$t = new TestCase('test_pool_callable_leaves_no_gc_root_in_task', 'asyncdecl');

// A $destroy closure, run by evict().
$withDestroy = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static function (object $r): void {
    },
    maxSize: 1,
);
$noDestroy = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
$result = asyncdecl_settle(oxphp_async(static function () use ($withDestroy, $noDestroy): array {
    $withDestroy->acquire()->release();
    $noDestroy->acquire()->release();
    $roots = gc_status()['roots'];
    $noDestroy->evict();
    $control = gc_status()['roots'] - $roots;
    $roots = gc_status()['roots'];
    $withDestroy->evict();
    return [$control, gc_status()['roots'] - $roots];
}));
$t->assertTrue('the $destroy task returns', $result[0]);
[$control, $added] = $result[1] + [0, null];
$t->assertSame('a $destroy closure run by evict() leaves no root', $added, $control);

// A factory closure, run by acquire().
$closureFactory = new \OxPHP\Shared\Pool(
    factory: static fn (): object => date_create(),
    destroy: null,
    maxSize: 1,
);
$builtinFactory = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
$result = asyncdecl_settle(oxphp_async(static function () use ($closureFactory, $builtinFactory): array {
    $closureFactory->stats();
    $builtinFactory->stats();
    $roots = gc_status()['roots'];
    $builtinFactory->acquire()->release();
    $control = gc_status()['roots'] - $roots;
    $roots = gc_status()['roots'];
    $closureFactory->acquire()->release();
    $added = gc_status()['roots'] - $roots;
    $closureFactory->evict();
    $builtinFactory->evict();
    return [$control, $added];
}));
$t->assertTrue('the factory task returns', $result[0]);
[$control, $added] = $result[1] + [0, null];
$t->assertSame('a factory closure run by acquire() leaves no root', $added, $control);

// [$closure, '__invoke'] as $destroy: the call reaches the closure itself.
$invokeDestroy = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: [static function (object $r): void {
    }, '__invoke'],
    maxSize: 1,
);
$noDestroy = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
$result = asyncdecl_settle(oxphp_async(static function () use ($invokeDestroy, $noDestroy): array {
    $invokeDestroy->acquire()->release();
    $noDestroy->acquire()->release();
    $roots = gc_status()['roots'];
    $noDestroy->evict();
    $control = gc_status()['roots'] - $roots;
    $roots = gc_status()['roots'];
    $invokeDestroy->evict();
    return [$control, gc_status()['roots'] - $roots];
}));
$t->assertTrue('the __invoke task returns', $result[0]);
[$control, $added] = $result[1] + [0, null];
$t->assertSame('the closure behind [$closure, \'__invoke\'] leaves no root', $added, $control);

// A closure made from a built-in function, date_create(...), as the factory.
$fakeFactory = new \OxPHP\Shared\Pool(factory: date_create(...), destroy: null, maxSize: 1);
$builtinFactory = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
$result = asyncdecl_settle(oxphp_async(static function () use ($fakeFactory, $builtinFactory): array {
    $fakeFactory->stats();
    $builtinFactory->stats();
    $roots = gc_status()['roots'];
    $builtinFactory->acquire()->release();
    $control = gc_status()['roots'] - $roots;
    $roots = gc_status()['roots'];
    $fakeFactory->acquire()->release();
    $added = gc_status()['roots'] - $roots;
    $fakeFactory->evict();
    $builtinFactory->evict();
    return [$control, $added];
}));
$t->assertTrue('the date_create(...) task returns', $result[0]);
[$control, $added] = $result[1] + [0, null];
$t->assertSame('a closure made from a built-in function leaves no root', $added, $control);

$t->done();
