<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A pool whose factory reaches __call() takes the function the engine built
// for it out of the thread's trampoline slot. The function built in that slot
// next, for a parent::$prop::set() call on a property without hooks, sets no
// attributes of its own, so the pool has to clear those of __call() as well:
// left behind, they hid an argument of that call in a backtrace wherever
// __call() marks its own as #[\SensitiveParameter], and without OPcache they
// were freed memory by the next request.
$t = new TestCase('test_pool_trampoline_leaves_no_attributes_in_the_slot', 'asyncdecl');

final class AsyncDeclTrampolineSensitiveMaker
{
    public function __call(#[\SensitiveParameter] string $name, array $args): object
    {
        return new \stdClass();
    }
}

class AsyncDeclTrampolineHookBase
{
    public int $n = 0;
}

final class AsyncDeclTrampolineHookChild extends AsyncDeclTrampolineHookBase
{
    public int $n {
        set(int|string $value) {
            parent::$n::set($value);
        }
    }
}

/** The first argument a backtrace shows for parent::$n::set(). */
$setArg = static function (): mixed {
    try {
        $child = new AsyncDeclTrampolineHookChild();
        $child->n = 'not a number';
    } catch (\TypeError $e) {
        foreach ($e->getTrace() as $frame) {
            if ($frame['function'] === '$n::set' && ($frame['class'] ?? null) === AsyncDeclTrampolineHookBase::class) {
                $arg = $frame['args'][0] ?? 'no args';
                return is_object($arg) ? $arg::class : $arg;
            }
        }
        return 'no parent::$n::set() frame in ' . implode(', ', array_column($e->getTrace(), 'function'));
    }
    return 'no TypeError';
};

$t->assertSame('before any pool, the backtrace shows the argument', $setArg(), 'not a number');

$pool = new \OxPHP\Shared\Pool(factory: [new AsyncDeclTrampolineSensitiveMaker(), 'make'], destroy: null, maxSize: 1);
$t->assertSame('once a pool took its factory out of the slot, it still does', $setArg(), 'not a number');
unset($pool);

$t->done();
