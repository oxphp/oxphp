<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A pool bound to a name outlives the worker that created it, and the code of
// its factory goes with that worker's memory when the code is not in OPcache's
// shared memory: eval()'d here. A slot that factory made on a task's thread
// held the literals of that code, an object whose properties are an array
// literal among them, and was handed out again after the worker had exited.
// Such a factory makes no slot there.
//
// ?action=trigger creates the pool, has tasks acquire from it, and has its
// worker exit.
//
// ?action=check runs on the worker that replaced it: it binds the name again,
// which gives the pool a factory of this worker, and has tasks acquire from it.

$marker = '/tmp/oxphp-asyncdecl-registry-outside-shm.json';

$t = new TestCase('test_pool_registry_callback_outside_shared_memory_in_task_outlives_its_creator', 'asyncdecl');

/**
 * Four tasks that each acquire() a slot of $pool, read its name and hold the
 * slot for a while, so both of the pool's threads take some: what each one's
 * acquire() did, ['returned', the name] or the class and message of what it
 * threw, or the task's failure.
 *
 * @return list<mixed>
 */
function asyncdecl_outside_shm_acquire_in_tasks(Pool $pool): array
{
    $promises = [];
    for ($i = 0; $i < 4; $i++) {
        $promises[] = oxphp_async(static function () use ($pool): array {
            try {
                $h = $pool->acquire();
            } catch (\Throwable $e) {
                return [$e::class, $e->getMessage()];
            }
            $name = $h->get()->name;
            usleep(200_000);
            $h->release();
            return ['returned', $name];
        });
    }
    return array_map(static fn (int $promise): mixed => asyncdecl_settle($promise)[1], $promises);
}

/** A pool of four slots, each an object whose properties are an array literal. */
$bind = static function (string $name): array {
    $factory = eval("return static fn (): object => (object) ['name' => 'from-the-literal'];");
    return [$factory, Registry::pool($name, static fn (): Pool => new Pool(factory: $factory, destroy: null, maxSize: 4))];
};

if (($_GET['action'] ?? '') === 'trigger') {
    if (is_file($marker)) {
        unlink($marker);
    }
    $name = 'asyncdecl-registry-outside-shm-' . bin2hex(random_bytes(4));
    [$factory, $pool] = $bind($name);
    $t->assertSame(
        'the tasks make no slot from the factory',
        asyncdecl_outside_shm_acquire_in_tasks($pool),
        array_fill(0, 4, asyncdecl_outside_shm($factory)),
    );
    file_put_contents($marker, json_encode(['name' => $name]));

    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$t->assertTrue('the trigger wrote the marker', is_file($marker));
$name = json_decode((string) @file_get_contents($marker), true)['name'] ?? 'asyncdecl-registry-outside-shm-missing';
[$factory, $pool] = $bind($name);
$t->assertSame(
    'the tasks get no slot of the worker that exited, and make none from the factory of this one',
    asyncdecl_outside_shm_acquire_in_tasks($pool),
    array_fill(0, 4, asyncdecl_outside_shm($factory)),
);

$t->done();
