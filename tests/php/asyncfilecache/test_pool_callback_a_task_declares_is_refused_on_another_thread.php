<?php

declare(strict_types=1);

require_once __DIR__ . '/../asyncdecl/helper.php';

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;

// With no shared memory, OPcache loads this script from its file cache into
// the memory of the request that runs it, and a task runs that code where it
// is. A pool the task creates with a closure it declares has its callback in
// the memory of this request, not of the task's thread, and refuses to run it
// on any thread but the task's.
$t = new TestCase('test_pool_callback_a_task_declares_is_refused_on_another_thread', 'asyncfilecache');

$map = new Map();
$line = __LINE__ + 3;
[$ok, $made] = asyncdecl_settle(oxphp_async(static function () use ($map): string {
    $map->set('pool', new Pool(
        factory: static fn (): object => (object) ['by' => 'the factory'],
        destroy: null,
        maxSize: 1,
    ));
    return 'made';
}));
$t->assertSame('the task created the pool', [$ok, $made], [true, 'made']);

$pool = $map->get('pool');
try {
    $h = $pool->acquire();
    $result = ['returned', $h->get()->by];
    $h->release();
} catch (\Throwable $e) {
    $result = [$e::class, $e->getMessage()];
}
$t->assertSame(
    "acquire() on this request's thread refuses the task's callback",
    $result,
    [\Error::class, 'The closure at ' . __FILE__ . ":$line" . ASYNCDECL_SHM_TAIL],
);

$t->done();
