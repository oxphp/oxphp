<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A $destroy naming a method its class does not have runs __call() through a
// function the engine frees once it has been called. The pool runs it for
// every slot it destroys, so the second one ran a freed function and took the
// whole process down.
$t = new TestCase('test_pool_trampoline_destroy_called_twice', 'asyncdecl');

final class AsyncDeclTrampolineDropper
{
    /** @var list<string> */
    public array $calls = [];

    public function __call(string $name, array $args): void
    {
        $this->calls[] = $name;
    }
}

$dropper = new AsyncDeclTrampolineDropper();
$pool = new \OxPHP\Shared\Pool(factory: 'date_create', destroy: [$dropper, 'drop'], maxSize: 2);
$a = $pool->acquire();
$b = $pool->acquire();
$a->release();
$b->release();

$t->assertSame('evict() destroys both idle slots', $pool->evict(), 2);
$t->assertSame('$destroy reached __call() once per slot', $dropper->calls, ['drop', 'drop']);

$t->done();
