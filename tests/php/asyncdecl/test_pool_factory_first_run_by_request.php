<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A factory that already ran on the worker thread is refused on the task's
// thread all the same: what the class lookup is checked against is the task
// thread's own class table, not what the factory's first run left behind.
$t = new TestCase('test_pool_factory_first_run_by_request', 'asyncdecl');

$line = __LINE__ + 2;
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new class { public int $x = 41; },
    destroy: null,
    maxSize: 2,
);

$h = $pool->acquire();
$t->assertSame('the factory runs on the worker thread', $h->get()->x, 41);

// The slot is held, so the task cannot be handed it and has to mint its own.
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    try {
        $th = $pool->acquire();
        $x = $th->get()->x;
        $th->release();
        return ['returned', $x];
    } catch (\Throwable $e) {
        return [$e::class, $e->getMessage()];
    }
}));
$h->release();

$t->assertTrue('the task returns', $ok);
$t->assertSame('acquire() in the task throws an Error', $result, [
    \Error::class,
    'Cannot declare anonymous class at ' . __FILE__ . ":$line" . ASYNCDECL_TAIL,
]);

$t->done();
