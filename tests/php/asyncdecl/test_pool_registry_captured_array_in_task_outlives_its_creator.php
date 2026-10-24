<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A pool bound to a name outlives the worker that created it, and what its
// factory and $destroy captured goes with that worker's memory. A task that ran
// them before the worker exited keeps nothing of what they captured on its
// thread: a collection there later reads nothing of the memory that went away.
//
// ?action=trigger creates the pool, whose callbacks capture an array, has a
// task run its factory and $destroy, and has its worker exit.
//
// ?action=check runs on the worker that replaced it: tasks collect garbage on
// both of the pool's threads, the one that ran the callbacks among them.

$marker = '/tmp/oxphp-asyncdecl-registry-captured-array.json';

$t = new TestCase('test_pool_registry_captured_array_in_task_outlives_its_creator', 'asyncdecl');

if (($_GET['action'] ?? '') === 'trigger') {
    if (is_file($marker)) {
        unlink($marker);
    }
    $name = 'asyncdecl-registry-captured-array-' . bin2hex(random_bytes(4));
    $config = ['dsn' => 'mem:' . random_int(1, 1), 'options' => [random_int(2, 2)]];
    $pool = Registry::pool($name, static fn (): Pool => new Pool(
        factory: static fn (): object => (object) ['dsn' => $config['dsn']],
        destroy: static function (object $r) use ($config): void {
        },
        maxSize: 1,
    ));
    // A value already in this thread's buffer of possible garbage roots is not
    // added to another's, and creating the pool can put the array there.
    gc_collect_cycles();
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
        $h = $pool->acquire();
        $dsn = $h->get()->dsn;
        $h->release();
        return [$dsn, $pool->evict(), $pool->stats()->size()];
    }));
    $t->assertTrue('the task returns', $ok);
    $t->assertSame('the task made a slot from the array and evict() dropped it', $result, ['mem:1', 1, 0]);
    file_put_contents($marker, json_encode(['name' => $name]));

    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$t->assertTrue('the trigger wrote the marker', is_file($marker));

// Four tasks that each hold their thread for a while, so both of the pool's
// threads take some.
$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(static function (): int {
        usleep(200_000);
        return gc_collect_cycles();
    });
}
$returned = 0;
foreach ($promises as $promise) {
    [$ok] = asyncdecl_settle($promise);
    $returned += $ok ? 1 : 0;
}
$t->assertSame('every task collected garbage and returned', $returned, 4);

$t->done();
