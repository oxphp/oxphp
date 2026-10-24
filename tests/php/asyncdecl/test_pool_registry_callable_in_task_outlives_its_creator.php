<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A pool bound to a name outlives the worker that created it, and its factory
// and $destroy go with that worker's memory. A task that ran them before the
// worker exited keeps nothing of them on its thread: a collection there later
// reads nothing of the memory that went away.
//
// ?action=trigger creates the pool, has a task run its factory and $destroy,
// and has its worker exit.
//
// ?action=check runs on the worker that replaced it: tasks collect garbage on
// both of the pool's threads, the one that ran the callbacks among them.

$marker = '/tmp/oxphp-asyncdecl-registry-callable.json';

$t = new TestCase('test_pool_registry_callable_in_task_outlives_its_creator', 'asyncdecl');

if (($_GET['action'] ?? '') === 'trigger') {
    if (is_file($marker)) {
        unlink($marker);
    }
    $name = 'asyncdecl-registry-callable-' . bin2hex(random_bytes(4));
    $pool = Registry::pool($name, static fn (): Pool => new Pool(
        factory: static fn (): object => new \stdClass(),
        destroy: static function (object $r): void {
        },
        maxSize: 1,
    ));
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
        $pool->acquire()->release();
        return [$pool->evict(), $pool->stats()->size()];
    }));
    $t->assertTrue('the task returns', $ok);
    $t->assertSame('the task made a slot and evict() dropped it', $result, [1, 0]);
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
