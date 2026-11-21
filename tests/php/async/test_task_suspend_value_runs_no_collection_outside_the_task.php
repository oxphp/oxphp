<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A task's Fiber::suspend() is refused on the async worker's scheduler, between
// the task's slices, where no PHP frame is on the stack, and the value the task
// passed to it is handed back there. Letting go of that value must not be what
// starts a garbage collection: a collection run there runs the destructors it
// finds outside any fiber, with no frame for what they throw to unwind to —
// which the engine answers by ending the process.
//
// Each phase leaves a garbage cycle holding a started generator whose finally
// block records where it ran, fills the collector's buffer of possible roots to
// a given distance from the point where the next one starts a collection, and
// then suspends with a fresh object each time. A generator rather than an
// object with a destructor: a class this script declares does not exist on the
// async worker's thread. The engine registers a value as a possible root when a
// reference to it is dropped and others remain, which both letting go of the
// suspended value and the task's next assignment over it do; whichever comes
// first is the one that fills the buffer.
//
// One root short, the value of the slice's first suspend is the one: the
// scheduler gets it as what it entered the fiber for. A few roots short, it is
// a later one, which it gets as it refuses the one before.

$t = new TestCase('task_suspend_value_runs_no_collection_outside_the_task', 'async');

$p = oxphp_async(static function (): array {
    $keep = [];
    $phase = static function (int $headroom) use (&$keep): array {
        gc_collect_cycles();
        $runsBefore = gc_status()['runs'];

        $where = new \ArrayObject();
        $o = new \stdClass();
        $g = (static function () use ($o, $where): \Generator {
            try {
                yield 1;
            } finally {
                $where['ran'] = \Fiber::getCurrent() === null ? 'outside any fiber' : 'in a fiber';
            }
        })();
        $g->current();
        $o->generator = $g;
        unset($o, $g);

        while (gc_status()['threshold'] - gc_status()['roots'] > $headroom) {
            $o = new \stdClass();
            $keep[] = $o;
            unset($o);
        }

        $refused = 0;
        for ($i = 0; $i < $headroom + 4; $i++) {
            $fresh = new \stdClass();
            $keep[] = $fresh;
            try {
                \Fiber::suspend($fresh);
            } catch (\FiberError) {
                $refused++;
            }
        }
        unset($fresh);

        gc_collect_cycles();

        return [
            'where' => $where['ran'] ?? 'never',
            'refused' => $refused,
            'runs' => gc_status()['runs'] - $runsBefore,
        ];
    };

    return ['first' => $phase(1), 'later' => $phase(5)];
});
$r = oxphp_async_await($p, 20.0);
$t->meta('task', $r);
foreach (['first' => 1, 'later' => 5] as $name => $headroom) {
    $t->assertSame("[{$name}] every suspend was refused", $r[$name]['refused'], $headroom + 4);
    $t->assertGreaterThan("[{$name}] the buffer filled up and was collected before the phase ended", $r[$name]['runs'], 1);
    $t->assertSame("[{$name}] the collection ran inside the task, not on the scheduler", $r[$name]['where'], 'in a fiber');
}
$t->done();
