<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory reaching __callStatic() that a task's acquire() runs twice, on the
// task's thread. The function that reaches __callStatic() is freed by the call
// it is made for, so the task's thread must not free the one the pool's
// creator has.
$t = new TestCase('test_pool_trampoline_factory_in_task', 'asyncdecl');

final class AsyncDeclTrampolineTaskMaker
{
    public static function __callStatic(string $name, array $args): object
    {
        $o = new \stdClass();
        $o->name = $name;
        return $o;
    }
}

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclTrampolineTaskMaker::class, 'make'],
    destroy: null,
    maxSize: 2,
);
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    $a = $pool->acquire();
    $b = $pool->acquire();
    $names = [$a->get()->name, $b->get()->name];
    $a->release();
    $b->release();
    return $names;
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('both slots the task minted came from __callStatic()', $result, ['make', 'make']);

$t->done();
