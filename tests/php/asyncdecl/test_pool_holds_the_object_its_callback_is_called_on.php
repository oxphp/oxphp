<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A callback named by its class, [self::class, 'make'] in a method of that
// class, is called on the $this of the code that created the pool, and one
// passed as [&$o, 'make'] on the object $o held then; neither callable holds
// that object. The pool does, until it is freed.
$t = new TestCase('test_pool_holds_the_object_its_callback_is_called_on', 'asyncdecl');

final class AsyncDeclHeldService
{
    public static int $destroyed = 0;

    /** @var list<int> */
    public array $payload;

    public function __construct()
    {
        $this->payload = array_fill(0, 64, 1);
    }

    public function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(factory: [self::class, 'make'], destroy: null, maxSize: 2);
    }

    public function make(): object
    {
        return (object) ['n' => count($this->payload)];
    }

    public function __destruct()
    {
        self::$destroyed++;
    }
}

/** Fill the memory a freed object would be reused from. */
function asyncdecl_churn(): array
{
    $junk = [];
    for ($i = 0; $i < 2000; $i++) {
        $junk[] = new \ArrayObject([$i, str_repeat('x', 40)]);
    }
    return $junk;
}

foreach ([
    '[self::class, \'make\'] in a method' => static function (): array {
        $svc = new AsyncDeclHeldService();
        $pool = $svc->pool();
        return [$pool, static function () use (&$svc): void { $svc = null; }];
    },
    '[&$o, \'make\']' => static function (): array {
        $o = new AsyncDeclHeldService();
        $pool = new \OxPHP\Shared\Pool(factory: [&$o, 'make'], destroy: null, maxSize: 2);
        return [$pool, static function () use (&$o): void { $o = null; }];
    },
] as $what => $make) {
    AsyncDeclHeldService::$destroyed = 0;
    [$pool, $letGo] = $make();
    $first = $pool->acquire();
    $t->assertSame("$what: the first factory call", $first->get()->n, 64);
    $letGo();
    unset($letGo, $make);
    gc_collect_cycles();
    $t->assertSame("$what: the object outlives the code's own reference to it", AsyncDeclHeldService::$destroyed, 0);
    if (AsyncDeclHeldService::$destroyed === 0) {
        $junk = asyncdecl_churn();
        $second = $pool->acquire();
        $t->assertSame("$what: the next factory call runs on the same object", $second->get()->n, 64);
        $second->release();
        unset($junk, $second);
    }
    $first->release();
    unset($first, $pool);
    gc_collect_cycles();
    $t->assertSame("$what: freeing the pool lets the object go", AsyncDeclHeldService::$destroyed, 1);
}

$t->done();
