<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A built-in function as the factory exists on every thread, and has no body
// to check: a task's acquire() runs it as before.
$t = new TestCase('test_pool_internal_factory_in_task', 'asyncdecl');

$pool = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: null, maxSize: 1);
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): string {
    $h = $pool->acquire();
    $class = $h->get()::class;
    $h->release();
    return $class;
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('acquire() hands out what date_create() built', $result, \DateTime::class);

$t->done();
