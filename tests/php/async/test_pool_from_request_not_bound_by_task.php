<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;
use OxPHP\Shared\SharedException;

// Outside worker mode a pool lives as long as the request that creates it, and
// a registry name lives as long as the server. A task the request hands its
// pool to cannot put that pool under a name either: neither registry method
// binds a pool the calling thread may not use, and the name stays free.
$t = new TestCase('test_pool_from_request_not_bound_by_task', 'async');

$run = bin2hex(random_bytes(6));
$pool = new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(), destroy: null, maxSize: 1);

foreach (['pool', 'global'] as $method) {
    $key = "async-request-pool-$method-$run";
    [$threw, $message] = oxphp_async_await(oxphp_async(function () use ($pool, $key, $method): array {
        try {
            Registry::$method($key, static fn (): Pool => $pool);
            return [null, ''];
        } catch (\Throwable $e) {
            return [get_class($e), $e->getMessage()];
        }
    }));
    $t->assertSame("Registry::$method() in a task refuses the request's pool", $threw, SharedException::class);
    $t->assertContains("the $method() refusal says why", $message, 'outside worker mode');
    $t->assertFalse("the $method() name stays free", in_array($key, Registry::keys(), true));
}

$h = $pool->acquire();
$t->assertInstanceOf('the pool still serves the request that made it', $h->get(), \ArrayObject::class);
$h->release();

$t->done();
