<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';
require_once __DIR__ . '/bulk.php';

// A pool whose factory is a function or a method named by a string, created
// after a task was started, and acquired from in that task. The task's thread
// never included the factory's file, which this worker thread included after
// a file of thousands of functions: OPcache numbered the factory's per-thread
// slots past the slots the task's thread had when the task reached it. The
// factory runs there, and the server stays up. The task's thread has taken the
// task up before this thread includes anything, so what covers the factory's
// slots there is the factory call, not the start of the task.
//
// In the second round the task first starts a task of its own and waits for
// it: that task is started from the task's thread, whose table is smaller
// than this one's.
$t = new TestCase('test_pool_named_factory_created_after_task_started', 'asyncdecl');

$rounds = [
    'a function' => ['pool_function', 'fixture_late_pool.php', 'asyncdecl_late_factory', false],
    'a method, after a task of its own' => ['pool_method', 'fixture_late_pool_method.php', 'AsyncDeclLateMaker::make', true],
];
foreach ($rounds as $round => [$tag, $file, $factory, $nested]) {
    $name = "asyncdecl_late_$tag";
    $started = new \OxPHP\Shared\Counter();
    $ready = new \OxPHP\Shared\Counter();
    $promise = oxphp_async(static function () use ($started, $ready, $name, $nested): array {
        $started->add();
        $deadline = hrtime(true) + 10_000_000_000;
        while ($ready->get() === 0) {
            if (hrtime(true) > $deadline) {
                return ['the pool was never bound'];
            }
            usleep(1000);
        }
        $own = $nested ? oxphp_async_await(oxphp_async(static fn (): int => 1)) : 1;
        $pool = \OxPHP\Shared\Registry::pool(
            $name,
            static fn (): never => throw new \LogicException('the pool is not bound'),
        );
        $h = $pool->acquire();
        $made = (array) $h->get();
        $h->release();
        return [$own, $made, $pool->evict()];
    });

    $deadline = hrtime(true) + 10_000_000_000;
    while ($started->get() === 0 && hrtime(true) < $deadline) {
        usleep(1000);
    }
    $startedFirst = $started->get() === 1;

    $bulk = asyncdecl_bulk($tag);
    $fixture = __DIR__ . "/$file";
    require_once $fixture;
    \OxPHP\Shared\Registry::pool(
        $name,
        static fn (): \OxPHP\Shared\Pool => new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1),
    );
    $ready->add();
    [$ok, $result] = asyncdecl_settle($promise);
    \OxPHP\Shared\Registry::remove($name);

    $t->assertTrue("$round: the task started before this thread included anything", $startedFirst);
    $t->assertTrue("$round: the generated file is cached", opcache_is_script_cached($bulk));
    $t->assertTrue("$round: the factory's file is cached", opcache_is_script_cached($fixture));
    $t->assertTrue("$round: the task returns", $ok);
    $t->assertSame("$round: the factory built its object, evict() dropped the slot",
        $result, [1, ['made' => 1, 'doubled' => [2, 4, 6]], 1]);
}

$t->done();
