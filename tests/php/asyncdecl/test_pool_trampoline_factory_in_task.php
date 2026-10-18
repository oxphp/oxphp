<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory reaching __call() that a task's acquire() runs, on the task's
// thread. The function that reaches __call() was the one the pool's creator
// had, and the task's thread freed it as if it were its own: the first
// acquire() took the whole process down.
$t = new TestCase('test_pool_trampoline_factory_in_task', 'asyncdecl');

final class AsyncDeclTrampolineTaskMaker
{
    public function __call(string $name, array $args): object
    {
        $o = new \stdClass();
        $o->name = $name;
        return $o;
    }
}

$pool = new \OxPHP\Shared\Pool(
    factory: [new AsyncDeclTrampolineTaskMaker(), 'make'],
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
$t->assertSame('both slots the task minted came from __call()', $result, ['make', 'make']);

$t->done();
