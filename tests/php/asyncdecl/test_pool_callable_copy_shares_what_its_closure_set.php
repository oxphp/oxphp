<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// The pool calls its own copy of a closure. A `static` that a call of the
// closure set before the pool was created, and a variable it captured by
// reference whose variable outside has gone away, are shared by the closure
// and the copy. A copy that did not share them crashed on its first call when
// the `static` has a non-constant initializer, and one run by a task put a
// reference of the task's thread in a table this request frees.
$t = new TestCase('test_pool_callable_copy_shares_what_its_closure_set', 'asyncdecl');

$initialized = static function (): object {
    static $made = random_int(0, 0);
    $o = new \stdClass();
    $o->made = ++$made;
    return $o;
};
$constant = static function (): object {
    static $made = 0;
    $o = new \stdClass();
    $o->made = ++$made;
    return $o;
};
$byReference = (static function (): \Closure {
    $made = 0;
    return static function () use (&$made): object {
        $o = new \stdClass();
        $o->made = ++$made;
        return $o;
    };
})();

foreach (['a static with a non-constant initializer' => $initialized, 'a static' => $constant, 'a capture by reference' => $byReference] as $what => $factory) {
    $factory();
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $h = $pool->acquire();
    $t->assertSame("$what: the pool's copy counts on from the closure's call", $h->get()->made, 2);
    $h->release();
    unset($h, $pool);
    $t->assertSame("$what: the closure counts on from the copy's call", $factory()->made, 3);
}

// The copy run by a task, and the pool then dropped by this request.
$factory = static function (): object {
    static $made = 0;
    $o = new \stdClass();
    $o->made = ++$made;
    return $o;
};
$factory();
$pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
[$ok, $made] = asyncdecl_settle(oxphp_async(static function () use ($pool): int {
    $h = $pool->acquire();
    $made = $h->get()->made;
    $h->release();
    return $made;
}));
$t->assertTrue('in a task: the task returns', $ok);
$t->assertSame("in a task: the pool's copy counts on from the closure's call", $made, 2);
unset($pool);
$t->assertSame('in a task: the closure counts on from the copy run by the task', $factory()->made, 3);

$t->done();
