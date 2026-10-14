<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_destroy_helper(object $r): void
{
}

// A task's evict() runs $destroy on the task's thread for the slots that thread
// holds idle. A $destroy that calls a function of this file is skipped the way
// one that throws is: the slot is dropped all the same and evict() counts it.
// On this worker thread, where the function exists, a $destroy of the same
// shape runs.
$t = new TestCase('test_pool_destroy_calls_function_in_task', 'asyncdecl');

$ran = new \OxPHP\Shared\Counter();
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new \stdClass(),
    destroy: static function (object $r) use ($ran): void {
        $ran->add();
        asyncdecl_destroy_helper($r);
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
    destroy: static function (object $r) use ($ranHere): void {
        $ranHere->add();
        asyncdecl_destroy_helper($r);
    },
    maxSize: 1,
);
$herePool->acquire()->release();

$t->assertSame('on this thread: evict() dropped the slot', [$herePool->evict(), $herePool->stats()->size()], [1, 0]);
$t->assertSame('on this thread: $destroy ran once', $ranHere->get(), 1);

$t->done();
