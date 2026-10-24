<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A closure keeps what its code looked up by name, a class among them, in a
// cache. The pool's copy of a factory shared that cache with every thread
// that ran it: a class of this script that the worker thread had found was
// used on the task's thread, which has no such class, and the task's
// acquire() took the process down. On a thread other than the one that
// created the pool, the factory looks the class up there.
$t = new TestCase('test_pool_callback_run_time_cache_is_per_thread', 'asyncdecl');

final class AsyncDeclCachedClass
{
    public int $v = 31;
}

$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new AsyncDeclCachedClass(),
    destroy: null,
    maxSize: 1,
);
$h = $pool->acquire();
$t->assertSame('the factory finds the class on the worker thread', $h->get()->v, 31);
$h->release();
$pool->evict();

$t->assertSame(
    'in a task the class is not found',
    asyncdecl_acquire_in_task($pool),
    [\Error::class, 'Class "AsyncDeclCachedClass" not found'],
);

$h = $pool->acquire();
$t->assertSame('the factory still finds it on the worker thread', $h->get()->v, 31);
$h->release();

$t->done();
