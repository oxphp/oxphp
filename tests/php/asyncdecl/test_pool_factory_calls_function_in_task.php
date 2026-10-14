<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_factory_helper(): object
{
    return new \stdClass();
}

// A task's acquire() on a pool whose factory calls a function of this file.
// The factory is refused before its first statement, so the counter stays at
// zero, and acquire() throws in the task.
$t = new TestCase('test_pool_factory_calls_function_in_task', 'asyncdecl');

$ran = new \OxPHP\Shared\Counter();
$line = __LINE__ + 4;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use ($ran): object {
        $ran->add();
        return asyncdecl_factory_helper();
    },
    destroy: null,
    maxSize: 1,
);
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    try {
        $pool->acquire()->release();
        return ['returned', ''];
    } catch (\Throwable $e) {
        return [$e::class, $e->getMessage()];
    }
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('acquire() throws an Error naming the function and its call site', $result, [
    \Error::class,
    'Call to undefined function asyncdecl_factory_helper() at ' . __FILE__ . ":$line" . ASYNCDECL_TAIL,
]);
$t->assertSame('the factory did not start', $ran->get(), 0);
$t->assertSame('no slot is left behind', $pool->stats()->size(), 0);

$h = $pool->acquire();
$t->assertInstanceOf('the factory still works on the worker thread', $h->get(), \stdClass::class);
$h->release();

$t->done();
