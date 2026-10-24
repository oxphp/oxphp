<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A `static` of a factory or $destroy that a task's acquire() or evict() set
// went into the table of the pool's copy of the closure, in memory of the
// task's thread, and this request, freeing that table, took the process down
// ("zend_mm_heap corrupted"). On a thread other than the one that created the
// pool, each call starts from the values the closure's variables had when the
// pool was created, in copies of them made on that thread, and what it sets
// is gone when it returns. On this thread the pool's copy and the closure go
// on sharing them.
$t = new TestCase('test_pool_callback_static_on_another_thread', 'asyncdecl');

/**
 * acquire() a slot of $pool twice in a task, evicting it after each: the
 * $property of each slot, or [false, the task's failure].
 */
function asyncdecl_twice_in_task(\OxPHP\Shared\Pool $pool, string $property): array
{
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool, $property): array {
        $seen = [];
        for ($i = 0; $i < 2; $i++) {
            $h = $pool->acquire();
            $seen[] = $h->get()->$property;
            $h->release();
            $pool->evict();
        }
        return $seen;
    }));
    return $ok ? $result : [false, $result];
}

// $destroy setting its `static` for the first time, run by evict().
$seen = new \OxPHP\Shared\Counter(0);
$pool = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static function (object $r) use ($seen): void {
        static $n = 0;
        $seen->add(++$n);
    },
    maxSize: 1,
);
[$ok] = asyncdecl_settle(oxphp_async(static function () use ($pool): bool {
    for ($i = 0; $i < 2; $i++) {
        $pool->acquire()->release();
        $pool->evict();
    }
    return true;
}));
$t->assertTrue('$destroy: the task returns', $ok);
$t->assertSame('$destroy: each call in the task starts from 0', $seen->get(), 2);
unset($pool);
$t->assertTrue('$destroy: the pool is gone', true);

// A `static` holding a string a call before the pool built, appended to.
$logging = static function (): object {
    static $log = '';
    $log .= str_repeat('x', 3);
    return (object) ['log' => $log];
};
$logging();
$pool = new \OxPHP\Shared\Pool(factory: $logging, destroy: null, maxSize: 1);
$t->assertSame('a string: each call in the task starts from it', asyncdecl_twice_in_task($pool, 'log'), ['xxxxxx', 'xxxxxx']);
unset($pool);
$t->assertSame('a string: the closure goes on from its own call', $logging()->log, 'xxxxxx');

// A `static` with an initializer that is not a constant, set before the pool,
// and one never set.
$initialized = static function (): object {
    static $made = random_int(0, 0);
    return (object) ['made' => ++$made];
};
$initialized();
$pool = new \OxPHP\Shared\Pool(factory: $initialized, destroy: null, maxSize: 1);
$t->assertSame('set before the pool: each call in the task starts from it', asyncdecl_twice_in_task($pool, 'made'), [2, 2]);
$h = $pool->acquire();
$t->assertSame("set before the pool: on this thread the pool's copy goes on from the closure", $h->get()->made, 2);
$h->release();
unset($h, $pool);
$t->assertSame('set before the pool: the closure goes on from the copy', $initialized()->made, 3);

$fresh = static function (): object {
    static $made = random_int(0, 0);
    return (object) ['made' => ++$made];
};
$pool = new \OxPHP\Shared\Pool(factory: $fresh, destroy: null, maxSize: 1);
$t->assertSame('never set: each call in the task sets it again', asyncdecl_twice_in_task($pool, 'made'), [1, 1]);
unset($pool);
$t->assertSame('never set: the closure sets it for itself', $fresh()->made, 1);

// A variable captured by reference is copied as it was when the pool was
// created.
$limit = 1;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use (&$limit): object {
        return (object) ['limit' => $limit];
    },
    destroy: null,
    maxSize: 1,
);
$limit = 2;
$t->assertSame('by reference: the task sees the value of the pool\'s creation', asyncdecl_twice_in_task($pool, 'limit'), [1, 1]);
$h = $pool->acquire();
$t->assertSame('by reference: this thread sees the variable', $h->get()->limit, 2);
$h->release();
unset($h, $pool);

// So is an OxPHP\Shared object captured by reference, after this request has
// put another in the variable and let go of the first.
$counter = new \OxPHP\Shared\Counter(5);
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use (&$counter): object {
        return (object) ['n' => $counter?->get()];
    },
    destroy: null,
    maxSize: 1,
);
$counter = new \OxPHP\Shared\Counter(7);
$t->assertSame('a Shared object by reference: the task sees the one of the pool\'s creation', asyncdecl_twice_in_task($pool, 'n'), [5, 5]);
$h = $pool->acquire();
$t->assertSame('a Shared object by reference: this thread sees the variable', $h->get()->n, 7);
$h->release();
unset($h, $pool);

// An arrow function binds each variable its body names, an arrow function's
// in it among them, and leaves one not defined here undefined, before one that
// is.
$after = 'b';
$names = static fn (): object => (object) [
    'values' => [$unset ?? 'none', $after, array_map(static fn (int $i): int => $i * 2, [1, 2])],
    'defined' => array_keys(get_defined_vars()),
];
$pool = new \OxPHP\Shared\Pool(factory: $names, destroy: null, maxSize: 1);
$t->assertSame('an arrow function: the task sees what it names', asyncdecl_twice_in_task($pool, 'values'), array_fill(0, 2, ['none', 'b', [2, 4]]));
$t->assertSame('an arrow function: what it names but this request did not define stays undefined', asyncdecl_twice_in_task($pool, 'defined'), array_fill(0, 2, $names()->defined));
$h = $pool->acquire();
$t->assertSame("an arrow function: on this thread the pool's copy leaves it undefined too", $h->get()->defined, $names()->defined);
$h->release();
unset($h, $pool);

$ran = new \OxPHP\Shared\Counter(0);
$pool = new \OxPHP\Shared\Pool(
    factory: 'date_create',
    destroy: static fn (object $r): int => $ran->add(count(array_map(static fn ($x) => $x, [$r]))),
    maxSize: 1,
);
$result = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $pool->acquire()->release();
    return $pool->evict();
}));
$t->assertSame('an arrow function as $destroy: the task evicts the slot', $result, [true, 1]);
$t->assertSame('an arrow function as $destroy: it runs in the task', $ran->get(), 1);
unset($pool);

$t->done();
