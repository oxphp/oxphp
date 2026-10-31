<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_enum_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// An enum's cases are created the first time they are read, from the enum
// looked up by its name, and every request that loads an enum opcache.preload
// did not load has a copy of its own. A callback that reads a case or a
// constant through self:: or static:: in an enum, or calls the enum's
// cases(), reaches the copy of the request that created the pool, and is
// refused on any other thread, one that declared an enum of the same name
// included. A constant read through static:: in a class that is not an enum
// creates nothing, and that class is not checked for it.
$t = new TestCase('test_pool_factory_reaches_an_enum_case_through_its_scope', 'asyncdecl');

enum AsyncDeclEnumConn: string
{
    case Primary = 'p';

    public function connect(): object
    {
        return (object) ['v' => $this->value];
    }

    public static function viaCase(): object
    {
        return self::Primary->connect();
    }

    public static function caseValue(): object
    {
        return (object) ['v' => static::Primary->value];
    }
}

class AsyncDeclEnumPlain
{
    public const N = 3;

    public static function build(): object
    {
        return (object) ['v' => asyncdecl_enum_scope_helper()];
    }

    public static function pool(): \OxPHP\Shared\Pool
    {
        // OPcache's optimizer replaces self::N with its value; static::N of
        // a class that is not final is read when the code runs.
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => (object) ['v' => static::N],
            destroy: null,
            maxSize: 1,
        );
    }
}

$refused = [\Error::class, 'Enum AsyncDeclEnumConn' . ASYNCDECL_ENUM_TAIL];
$cases = [
    'self::Primary->connect()' => [[AsyncDeclEnumConn::class, 'viaCase'], $refused],
    'static::Primary->value' => [[AsyncDeclEnumConn::class, 'caseValue'], $refused],
    'the enum\'s cases()' => [[AsyncDeclEnumConn::class, 'cases'], $refused],
];
foreach ($cases as $what => [$factory, $expected]) {
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool), $expected);
}

$t->assertSame(
    'static::N of a class that is not an enum runs in the task',
    asyncdecl_acquire_in_task(AsyncDeclEnumPlain::pool()),
    ['returned', \stdClass::class],
);

// The task's thread declares an enum of the same name first. Run there, the
// factory would create the case in this request's copy, from that thread's.
$conn = <<<'PHP'
    enum AsyncDeclEnumConn: string
    {
        case Primary = 'p';
    }
    PHP;
$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclEnumConn::class, 'viaCase'], destroy: null, maxSize: 1);
$t->assertSame(
    'self::Primary->connect(), on a thread that declared an enum of the same name: in a task',
    asyncdecl_settle(oxphp_async(static function () use ($pool, $conn): array {
        if (!\enum_exists('AsyncDeclEnumConn', false)) {
            eval($conn);
        }
        try {
            $h = $pool->acquire();
        } catch (\Throwable $e) {
            return [$e::class, $e->getMessage()];
        }
        $class = $h->get()::class;
        $h->release();
        $pool->evict();
        return ['returned', $class];
    })),
    [true, $refused],
);
$t->assertTrue(
    'the case the worker thread reads is one of its own enum',
    AsyncDeclEnumConn::Primary instanceof AsyncDeclEnumConn,
);

$h = (new \OxPHP\Shared\Pool(factory: [AsyncDeclEnumConn::class, 'viaCase'], destroy: null, maxSize: 1))->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 'p');
$h->release();

$t->done();
