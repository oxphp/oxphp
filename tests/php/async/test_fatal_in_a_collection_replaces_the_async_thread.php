<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A fatal that interrupts a cycle collection on an async worker thread leaves the
// collector unusable for the rest of that thread's life: the run it interrupted
// is still marked as running, so every later collection returns at once. The
// thread is replaced, at a moment it runs no task, and the next task runs on a
// thread whose collector works.
//
// The fatal is a compile error in the finally block of a generator that only a
// collection destroys: the generator and an object refer to each other. A
// generator rather than an object with a destructor: a class this script
// declares does not exist on the async worker's thread.

$t = new TestCase('fatal_in_a_collection_replaces_the_async_thread', 'async');

$p = oxphp_async(static function (): int {
    $o = new \stdClass();
    $g = (static function () use ($o): \Generator {
        try {
            yield 1;
        } finally {
            eval('abstract class OxphpCollectionFatal { abstract public function f() {} }');
        }
    })();
    $g->current();
    $o->generator = $g;
    unset($o, $g);

    return gc_collect_cycles();
});
$failure = '';
try {
    oxphp_async_await($p);
} catch (\Throwable $e) {
    $failure = $e::class . ': ' . $e->getMessage();
}
$t->assertContains('the task failed on the fatal in the finally block the collection ran', $failure, 'cannot contain body');

$after = oxphp_async_await(oxphp_async(static function (): array {
    $status = gc_status();
    $a = new \stdClass();
    $b = new \stdClass();
    $a->peer = $b;
    $b->peer = $a;
    unset($a, $b);

    return ['runs' => $status['runs'], 'running' => $status['running'], 'collected' => gc_collect_cycles()];
}));
$t->assertSame('the next task runs on a thread that never collected', $after['runs'], 0);
$t->assertFalse('whose collector is not stuck in a run', $after['running']);
$t->assertSame('and collects a cycle', $after['collected'], 2);

$t->done();
