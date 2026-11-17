<?php

declare(strict_types=1);

// A fiber the application starts keeps what the engine holds for the
// serialize(), unserialize() and sort calls it is suspended inside, whichever
// request resumes it and whatever runs on the worker meanwhile. The engine
// holds that per thread, for the stack that is running, and every fiber is a
// stack of its own.
//
// Three shapes:
// - a fiber suspended inside unserialize() by a request that then ends, resumed
//   by the next request, which unserializes on its own afterwards;
// - a fiber suspended in the comparator of its sort, resumed by another
//   request while the request that started it is parked;
// - a fiber suspended inside unserialize() while the request that started it
//   unserializes on its own.
//
// The first two hold where every fiber shares the thread's cells too: they pin
// that what the worker resets between requests leaves a suspended fiber's call
// alone. Only the third needs each fiber to keep a set of its own.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

ini_set('display_errors', '0');

$fixture = '/tests/fibers/fixture_userland_fiber_cells.php';

$suspended = json_decode(thread_cell_read(thread_cell_send("$fixture?step=suspend"))['body'], true);
$resumed = json_decode(thread_cell_read(thread_cell_send("$fixture?step=resume"))['body'], true);

ThreadCellFiberHolder::$sort = new Fiber(static function (): array {
    $values = [5, 3, 8, 1, 9, 2, 7, 4, 6];
    $first = true;
    usort($values, static function (int $a, int $b) use (&$first): int {
        if ($first) {
            $first = false;
            Fiber::suspend();
        }

        return $a <=> $b;
    });

    return $values;
});
ThreadCellFiberHolder::$sort->start();
$sortSuspended = ThreadCellFiberHolder::$sort->isSuspended();
$sort = json_decode(thread_cell_read(thread_cell_send("$fixture?step=sort"))['body'], true);

$own = new stdClass();
$own->who = 'fiber';
set_error_handler(static function (): bool {
    Fiber::suspend();

    return true;
});
$fiber = new Fiber(static fn () => unserialize(serialize([$own]) . 'X'));
$fiber->start();
restore_error_handler();
$besideSuspended = $fiber->isSuspended();
$beside = thread_cell_unserialize_probe();
$fiber->resume();
$besideResult = $fiber->isTerminated() ? $fiber->getReturn() : null;

$t = new TestCase('userland_fiber_keeps_its_call_cells', 'fibers');
$t->meta('suspended', $suspended);
$t->meta('resumed', $resumed);
$t->meta('sort', $sort);
$t->meta('beside', $beside);

$t->assertSame('a fiber suspended inside unserialize()', $suspended['suspended'] ?? null, true);
$t->assertSame('the next request resumed it and its call finished', $resumed['result_who'] ?? null, 'fiber');
$t->assertTrue(
    'and that request then unserialized on a table of its own',
    ($resumed['after']['same'] ?? false) === true && ($resumed['after']['filled'] ?? false) === true
);

$t->assertTrue('a fiber suspended in the comparator of its sort', $sortSuspended);
$t->assertNull('another request resumed it and the sort ended without an error', is_array($sort) && array_key_exists('error', $sort) ? $sort['error'] : 'no answer');
$t->assertSame('and sorted', $sort['sorted'] ?? null, [1, 2, 3, 4, 5, 6, 7, 8, 9]);

$t->assertTrue('a fiber suspended inside unserialize() beside the request', $besideSuspended);
$t->assertTrue(
    'the request unserialized on a table of its own meanwhile',
    $beside['same'] === true && $beside['filled'] === true
);
$t->assertSame(
    'and the suspended call then finished on its own',
    is_array($besideResult) && isset($besideResult[0]) && is_object($besideResult[0]) ? ($besideResult[0]->who ?? null) : null,
    'fiber'
);

$t->done();
