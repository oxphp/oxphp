<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// What a task passes to a Fiber::suspend() the server refuses is handed to the
// async worker's scheduler, and once the call returns, the scheduler's is the
// last reference to it outside any cycle it closes. The call's frame drops its
// argument without registering it as a possible root, as the engine does for
// every internal call: registering is left to whoever receives the value.
// Unregistered, a cycle no collection holds in its buffer is never collected.
//
// The value comes out of an SplQueue, which moves it out rather than dropping a
// reference, after a collection that found it alive and so took it off the
// buffer, so nothing on its way to the suspend registers it again. With
// zend.exception_ignore_args off, the FiberError's trace would hold the value
// and register it when the error is freed; on, as in production, nothing does.
//
// The first value is what the scheduler entered the fiber for, the slice's
// first suspend. The second it gets as it refuses the one before it.

$t = new TestCase('task_suspend_value_left_in_a_cycle_is_collected', 'async');

$p = oxphp_async(static function (): array {
    ini_set('zend.exception_ignore_args', '1');
    $queue = new \SplQueue();
    $cycle = static function () use ($queue): \WeakReference {
        $o = new \stdClass();
        $o->self = $o;
        $queue->enqueue($o);

        return \WeakReference::create($o);
    };
    $refused = 0;

    $first = $cycle();
    gc_collect_cycles();
    try {
        \Fiber::suspend($queue->dequeue());
    } catch (\FiberError) {
        $refused++;
    }
    gc_collect_cycles();
    $firstCollected = $first->get() === null;

    // A park of the task's own, so the next suspend opens a slice.
    oxphp_sleep(0.01);

    $second = $cycle();
    gc_collect_cycles();
    try {
        \Fiber::suspend();
    } catch (\FiberError) {
        $refused++;
    }
    try {
        \Fiber::suspend($queue->dequeue());
    } catch (\FiberError) {
        $refused++;
    }
    gc_collect_cycles();

    return [
        'refused' => $refused,
        'first' => $firstCollected,
        'second' => $second->get() === null,
    ];
});
$r = oxphp_async_await($p, 10.0);
$t->meta('task', $r);
$t->assertSame('every suspend was refused', $r['refused'], 3);
$t->assertTrue('the value of the slice\'s first suspend was collected with its cycle', $r['first']);
$t->assertTrue('the value of a suspend refused after another was collected with its cycle', $r['second']);
$t->done();
