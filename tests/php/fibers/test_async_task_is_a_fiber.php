<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('async_task_is_a_fiber', 'fibers');

// A background task is a logical task of its own, so it must have a fiber
// identity of its own — otherwise everything that keys on the current fiber
// (event loops, context storages, fiber-locals) files the task under whatever
// ran last.
//
// Everything the tasks report is measured on the async worker thread. Object
// ids are per-thread under ZTS, so the two tasks are comparable with each other
// — this profile runs one async worker, so both land on the same thread — but
// not with anything the request itself sees.
$a = oxphp_async(static function (): array {
    $before = \Fiber::getCurrent();
    oxphp_sleep(0.05);
    $after = \Fiber::getCurrent();

    return [
        'id' => $before !== null ? spl_object_id($before) : 0,
        'running' => $before !== null && $before->isRunning(),
        'same' => $before !== null && $after === $before,
    ];
});
$b = oxphp_async(static function (): array {
    $before = \Fiber::getCurrent();
    oxphp_sleep(0.05);

    return ['id' => $before !== null ? spl_object_id($before) : 0];
});

$ra = oxphp_async_await($a, 5.0);
$rb = oxphp_async_await($b, 5.0);

$t->assertGreaterThan('task A has a fiber', $ra['id'], 0);
$t->assertGreaterThan('task B has a fiber', $rb['id'], 0);
$t->assertNotEqual('two concurrent tasks are different fibers', $ra['id'], $rb['id']);

// A task's fiber is the one the engine is running, not some object that merely
// exists, and it is still the same one after the task parks and is resumed.
$t->assertTrue('the task fiber reports itself running', $ra['running']);
$t->assertTrue('the task keeps its fiber across a suspend', $ra['same']);

$t->done();
