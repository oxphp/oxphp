<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_builtin_static_helper(): void
{
}

// A built-in static method called on a class of the script can create an
// object of that class: DateTime::createFromInterface() creates one of the
// class it is called on. The $destroy below throws that object away, which
// runs its __destruct(). A task's evict() skips such a $destroy, as it does
// one that calls a function of this file itself, and drops the slot all the
// same. On this worker thread, where the function exists, the $destroy runs.
$t = new TestCase('test_pool_destroy_reaches_a_user_class_through_a_builtin_static_method', 'asyncdecl');

// The destructor looks its counter up by name: a static property is per thread.
class AsyncDeclDate extends \DateTime
{
    public function __destruct()
    {
        asyncdecl_builtin_static_helper();
        \OxPHP\Shared\Registry::counter(
            'asyncdecl-builtin-static-destructed',
            static fn (): \OxPHP\Shared\Counter => new \OxPHP\Shared\Counter(),
        )->add();
    }
}

$destructed = \OxPHP\Shared\Registry::counter(
    'asyncdecl-builtin-static-destructed',
    static fn (): \OxPHP\Shared\Counter => new \OxPHP\Shared\Counter(),
);
$before = $destructed->get();

$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new \DateTimeImmutable(),
    destroy: [AsyncDeclDate::class, 'createFromInterface'],
    maxSize: 1,
);
[$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
    $pool->acquire()->release();
    $idle = $pool->stats()->idle();
    return [$idle, $pool->evict(), $pool->stats()->size()];
}));

$t->assertTrue('the task returns', $ok);
$t->assertSame('the task\'s slot was idle, evict() dropped it, and none is left', $result, [1, 1, 0]);
$t->assertSame('no AsyncDeclDate was destructed in the task', $destructed->get() - $before, 0);

$pool->acquire()->release();
$t->assertSame('on this thread: evict() dropped the slot', [$pool->evict(), $pool->stats()->size()], [1, 0]);
$t->assertSame('on this thread: $destroy created an AsyncDeclDate and it was destructed', $destructed->get() - $before, 1);

$t->done();
