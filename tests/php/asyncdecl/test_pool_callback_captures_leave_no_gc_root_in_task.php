<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// What a factory or $destroy captured, and a `static` it set, live in the
// memory of this request. A task's acquire() or evict() that ran the callback
// with them left each in the task thread's buffer of possible garbage roots,
// as the callback's own closure once was, and the collection that reads such
// an entry takes the server down at a later request. On a thread other than
// the one that created the pool, the callback runs with copies of its
// closure's variables, made there.
//
// Each task counts the roots its thread gains while the callback runs, and the
// roots it gains while a pool of the same shape whose callback is a built-in
// function runs. The factories turn what they captured into the slot's
// timestamp, which the task returns. This thread collects garbage before each
// task: a value already in this thread's buffer is not added to another's,
// and creating the pool can put it there.
$t = new TestCase('test_pool_callback_captures_leave_no_gc_root_in_task', 'asyncdecl');

/**
 * acquire() a slot of $pool in a task, and one of a pool of date_create():
 * [the roots the date_create() pool added, the roots $pool added, the
 * timestamp of $pool's slot], or [false, the task's failure].
 *
 * @return array{mixed, mixed, ?int}
 */
function asyncdecl_factory_roots(\OxPHP\Shared\Pool $pool): array
{
    $builtin = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
    gc_collect_cycles();
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool, $builtin): array {
        $pool->stats();
        $builtin->stats();
        $roots = gc_status()['roots'];
        $b = $builtin->acquire();
        $control = gc_status()['roots'] - $roots;
        $roots = gc_status()['roots'];
        $h = $pool->acquire();
        $added = gc_status()['roots'] - $roots;
        $timestamp = $h->get()->getTimestamp();
        $h->release();
        $b->release();
        $pool->evict();
        $builtin->evict();
        return [$control, $added, $timestamp];
    }));
    return $ok ? $result : [false, $result, null];
}

$cases = [];

// An array built at run time.
$config = ['ts' => random_int(86400, 86400)];
$cases['an array'] = [
    new \OxPHP\Shared\Pool(
        factory: static fn (): object => date_create('@' . $config['ts']),
        destroy: null,
        maxSize: 1,
    ),
    86400,
];

// The same, captured by reference.
$shared = ['ts' => random_int(172800, 172800)];
$cases['an array captured by reference'] = [
    new \OxPHP\Shared\Pool(
        factory: static function () use (&$shared): object {
            return date_create('@' . $shared['ts']);
        },
        destroy: null,
        maxSize: 1,
    ),
    172800,
];

// An OxPHP\Shared object, which the copy shares: what the task adds is seen here.
$counter = new \OxPHP\Shared\Counter(3);
$cases['an OxPHP\Shared object'] = [
    new \OxPHP\Shared\Pool(
        factory: static fn (): object => date_create('@' . $counter->add(1) * 86400),
        destroy: null,
        maxSize: 1,
    ),
    4 * 86400,
];

// A `static` set to an array by a call before the pool was created.
$cached = static function (): object {
    static $base = null;
    $base ??= ['ts' => random_int(432000, 432000)];
    return date_create('@' . $base['ts']);
};
$cached();
$cases['a static set to an array'] = [
    new \OxPHP\Shared\Pool(factory: $cached, destroy: null, maxSize: 1),
    432000,
];

foreach ($cases as $what => [$pool, $timestamp]) {
    [$control, $added, $seen] = asyncdecl_factory_roots($pool);
    $t->assertTrue("$what: the task returns", $control !== false);
    $t->assertSame("$what: the factory run by acquire() leaves no root", $added, $control);
    $t->assertSame("$what: the factory sees what it captured", $seen, $timestamp);
}
$t->assertSame('the task added to the OxPHP\Shared object', $counter->get(), 4);

// A $destroy that captured an array, run by evict(). An array of its own: the
// first case left $config in the buffer of the thread that ran it.
$options = ['retries' => random_int(3, 3)];
$withDestroy = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static function (object $r) use ($options): void {
    },
    maxSize: 1,
);
$noDestroy = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
gc_collect_cycles();
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
$t->assertSame('a $destroy that captured an array, run by evict(), leaves no root', $added, $control);

$t->done();
