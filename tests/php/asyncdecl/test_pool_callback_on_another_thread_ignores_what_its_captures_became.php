<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_captures_became_helper(int $base = 0): int
{
    return $base + 1;
}

// On a thread other than the one that created the pool, a closure runs with
// copies of its variables as they were when the pool was created, and only
// those are looked at there: the request that created the pool can change a
// variable captured by reference while another thread runs the closure.
$t = new TestCase('test_pool_callback_on_another_thread_ignores_what_its_captures_became', 'asyncdecl');

final class AsyncDeclCapturesBecameMaker
{
    public function make(): object
    {
        return (object) ['v' => asyncdecl_captures_became_helper()];
    }
}

$maker = null;
$pool = new \OxPHP\Shared\Pool(
    factory: static function () use (&$maker): object {
        return $maker?->make() ?? new \stdClass();
    },
    destroy: null,
    maxSize: 1,
);
$maker = new AsyncDeclCapturesBecameMaker();

$t->assertSame(
    'in a task the factory runs with the value the variable had',
    asyncdecl_acquire_in_task($pool),
    ['returned', \stdClass::class],
);

$h = $pool->acquire();
$t->assertSame('on the worker thread the factory sees what the variable holds now', $h->get()->v, 1);
$h->release();

$t->done();
