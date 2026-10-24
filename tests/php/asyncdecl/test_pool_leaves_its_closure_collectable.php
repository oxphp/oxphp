<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// The pool keeps a copy of a closure it is given and calls that copy. The
// closure itself stays the caller's: one that captured a reference to itself
// is a garbage cycle once the pool and the variable holding it are gone, and
// the collector frees it.
$t = new TestCase('test_pool_leaves_its_closure_collectable', 'asyncdecl');

$f = static function () use (&$f): object {
    return new \stdClass();
};
$pool = new \OxPHP\Shared\Pool(factory: $f, destroy: null, maxSize: 1);
$h = $pool->acquire();
$t->assertSame('the pool runs the closure', $h->get()::class, \stdClass::class);
$h->release();
unset($h, $pool);

gc_collect_cycles();
unset($f);
$t->assertTrue('the closure and the reference it captured are collected', gc_collect_cycles() > 0);

$t->done();
