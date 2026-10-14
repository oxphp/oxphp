<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task's withTimeout() on a pool of the worker's entry file whose factory
// builds an anonymous class. The factory runs on the task's thread, which never
// ran that file: withTimeout() must throw there, and the slot it was minting is
// given back.
$t = new TestCase('test_pool_factory_anonymous_class_in_task', 'asyncdecl');

[$line, $pool] = $fixtures['pool'];
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    try {
        return ['returned', $pool->withTimeout(static fn (object $r): int => $r->n, 1000)];
    } catch (\Throwable $e) {
        return [$e::class, $e->getMessage()];
    }
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('withTimeout() throws an Error naming where the class is declared', $result, [
    \Error::class,
    "Cannot declare anonymous class at /var/www/html/asyncdecl/worker.php:$line" . ASYNCDECL_TAIL,
]);
$t->assertSame('no slot is left behind', $pool->stats()->size(), 0);

$t->done();
