<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_destroy_reach_helper(object $r): void
{
}

// A $destroy calling only a method of an object it captured, and that method
// calls a function of this file. A task's evict() runs $destroy on the task's
// thread, which gets no copy of the object: $destroy is skipped the way one
// that throws is, and the slot is dropped all the same. On this worker thread,
// where the function exists, a $destroy of the same shape runs.
$t = new TestCase('test_pool_destroy_reaches_function_through_captured_object', 'asyncdecl');

final class AsyncDeclDestroyCloser
{
    public function close(object $r): void
    {
        asyncdecl_destroy_reach_helper($r);
    }
}

$closer = new AsyncDeclDestroyCloser();
$ran = new \OxPHP\Shared\Counter();
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new \stdClass(),
    destroy: static function (object $r) use ($ran, $closer): void {
        $ran->add();
        $closer->close($r);
    },
    maxSize: 1,
);
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    $pool->acquire()->release();
    $idle = $pool->stats()->idle();
    return [$idle, $pool->evict(), $pool->stats()->size()];
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('the task\'s slot was idle, evict() dropped it, and none is left', $result, [1, 1, 0]);
$t->assertSame('$destroy did not start', $ran->get(), 0);

// The slot is minted and evicted on this worker thread, not in a task: a
// $destroy that a task's evict() does run crashes the server at a later
// request, for a reason that has nothing to do with this test.
$ranHere = new \OxPHP\Shared\Counter();
$herePool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new \stdClass(),
    destroy: static function (object $r) use ($ranHere, $closer): void {
        $ranHere->add();
        $closer->close($r);
    },
    maxSize: 1,
);
$herePool->acquire()->release();

$t->assertSame('on this thread: evict() dropped the slot', [$herePool->evict(), $herePool->stats()->size()], [1, 0]);
$t->assertSame('on this thread: $destroy ran once', $ranHere->get(), 1);

$t->done();
