<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_enum_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// An enum's cases are created on each thread the first time they are read,
// and creating one looks the enum up by its name. A callback that reads a case
// or a constant through self:: or static:: in an enum, or calls the enum's
// cases(), is refused on a thread whose tables do not hold the enum. A
// constant read through static:: in a class that is not an enum creates
// nothing, and that class is not checked for it.
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

$refused = [\Error::class, 'Enum AsyncDeclEnumConn' . ASYNCDECL_TAIL];
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

$h = (new \OxPHP\Shared\Pool(factory: [AsyncDeclEnumConn::class, 'viaCase'], destroy: null, maxSize: 1))->acquire();
$t->assertSame('the factory still works on the worker thread', $h->get()->v, 'p');
$h->release();

$t->done();
