<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_run_time_helper(int $base = 0): int
{
    return $base + 1;
}

// A callable naming its class as self, static or parent, built while the code
// runs, cannot be found by reading the callback before it is called. It is
// checked when PHP resolves it: on a thread that lacks that class, the class is
// checked whole and the call throws an Error naming what is missing. A
// call_user_func() compiled to an opcode throws a TypeError on top of that Error,
// as it does for a handler that throws.
// str_rot13() keeps the name out of the file's literals, so the check made before
// the call has nothing to find, and array_rand(), which OPcache's optimizer cannot
// evaluate, keeps it out of the compiled code as well.
//
// PHP deprecates these callables, and TestCase turns the notice into an
// exception whatever @ says, so each is called with deprecations ignored.
$t = new TestCase('test_pool_factory_reaches_scope_through_a_run_time_callable', 'asyncdecl');

/**
 * What an acquire in a task throws, each exception with those before it.
 *
 * @return array<array{string, string}>
 */
function asyncdecl_run_time_thrown_in_task(\OxPHP\Shared\Pool $pool): array
{
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
        $thrown = [];
        try {
            $pool->acquire()->release();
            $pool->evict();
        } catch (\Throwable $e) {
            for (; $e !== null; $e = $e->getPrevious()) {
                $thrown[] = [$e::class, $e->getMessage()];
            }
        }
        return $thrown;
    }));
    return $ok ? $result : ['the task failed', $result];
}

$makeLine = __LINE__ + 5;
class AsyncDeclRunTimeMaker
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_run_time_helper()];
    }

    public static function staticString(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }

    public static function selfString(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['frys::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }

    public static function staticArray(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func([\str_rot13(\array_rand(['fgngvp' => true])), 'make']);
        } finally {
            \restore_error_handler();
        }
    }

    // A built-in function that resolves the callable as it takes its arguments.
    public static function throughArrayMap(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \array_map(\str_rot13(\array_rand(['fgngvp::znxr' => true])), [1])[0];
        } finally {
            \restore_error_handler();
        }
    }

    public static function throughForwardStaticCall(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \forward_static_call(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }

    // filter_var() checks the callable with deprecations suppressed, so PHP resolves
    // it for the check that counts when it calls it by name.
    public static function throughFilterVar(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \filter_var('x', FILTER_CALLBACK, ['options' => \str_rot13(\array_rand(['fgngvp::znxr' => true]))]);
        } finally {
            \restore_error_handler();
        }
    }

    // filter_var() calls call_user_func(), which resolves the callable as it takes
    // its arguments, so the Error is pending when control is back in filter_var().
    public static function throughCallUserFuncInFilterVar(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \filter_var(\str_rot13(\array_rand(['fgngvp::znxr' => true])), FILTER_CALLBACK, ['options' => 'call_user_func']);
        } finally {
            \restore_error_handler();
        }
    }

    // PHP 8.4 keeps the callable and resolves it at the first notification; 8.5
    // resolves it in stream_context_create().
    public static function throughStreamNotifier(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            $context = \stream_context_create(params: ['notification' => \str_rot13(\array_rand(['fgngvp::znxr' => true]))]);
            \file_get_contents('http://127.0.0.1:9090/health', false, $context);
            return (object) ['v' => 1];
        } finally {
            \restore_error_handler();
        }
    }

    // The class self stands for here is not the one $this belongs to.
    public static function selfOfAClosureBoundElsewhere(): object
    {
        $call = \Closure::bind(function (): object {
            return \call_user_func(\str_rot13(\array_rand(['frys::znxr' => true])));
        }, new \stdClass());
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return $call();
        } finally {
            \restore_error_handler();
        }
    }

    // With a built-in function on top, self stands for the class of the PHP frame
    // below it, which only the class of the closure tells here.
    public static function throughArrayMapInAClosureBoundElsewhere(): object
    {
        $call = \Closure::bind(function (): object {
            return \array_map(\str_rot13(\array_rand(['frys::znxr' => true])), [1])[0];
        }, new \stdClass());
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return $call();
        } finally {
            \restore_error_handler();
        }
    }

    // The deprecation is not reported at this level, and the check runs all the same.
    public static function underErrorReportingZero(): object
    {
        $level = \error_reporting(0);
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
            \error_reporting($level);
        }
    }

    public static function destroyString(object $slot): void
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }

    public static function fromMap(\OxPHP\Shared\Map $map): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(factory: static function () use ($map): object {
            $map->set('reached the call', true);
            \set_error_handler(static function () use ($map): bool {
                $map->set('the handler ran', true);
                return true;
            }, E_DEPRECATED);
            try {
                return \call_user_func($map->get('callable'));
            } finally {
                \restore_error_handler();
            }
        }, destroy: null, maxSize: 1);
    }
}

final class AsyncDeclRunTimeChild extends AsyncDeclRunTimeMaker
{
    public static function parentString(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['cnerag::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }
}

// static here is a subclass: the class of the method calling it has nothing missing.
class AsyncDeclRunTimeBase
{
    public static function make(): object
    {
        return (object) ['v' => 2];
    }

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }
}

$subclassMakeLine = __LINE__ + 5;
final class AsyncDeclRunTimeSubclass extends AsyncDeclRunTimeBase
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_run_time_helper()];
    }
}

// Nothing in this class is missing anywhere, so it runs on any thread.
final class AsyncDeclRunTimeClean
{
    public static function make(): object
    {
        return (object) ['v' => 7];
    }

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }
}

// The class is checked whole, so a method the call does not take counts too.
$anonymousLine = __LINE__ + 10;
final class AsyncDeclRunTimeWhole
{
    public static function make(): object
    {
        return (object) ['v' => 8];
    }

    public static function other(): object
    {
        return new class () {};
    }

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(\str_rot13(\array_rand(['fgngvp::znxr' => true])));
        } finally {
            \restore_error_handler();
        }
    }
}

// Code of a class another thread lacks, doing what is not a callable being
// resolved: each runs up to its own return there.
final class AsyncDeclRunTimeNotACallable
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_run_time_helper()];
    }

    public static function sameTextFromUserCode(): object
    {
        \set_error_handler(static fn (): bool => true, E_USER_DEPRECATED);
        try {
            \trigger_error('Use of "static" in callables is deprecated', E_USER_DEPRECATED);
            return new \stdClass();
        } finally {
            \restore_error_handler();
        }
    }

    public static function anotherDeprecation(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            \utf8_encode('x');
            return new \stdClass();
        } finally {
            \restore_error_handler();
        }
    }
}

// Forms PHP resolves by the name a class is declared under, where it finds none.
final class AsyncDeclRunTimeByName
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_run_time_helper()];
    }

    public static function dynamicCall(): object
    {
        $callable = \str_rot13(\array_rand(['fgngvp::znxr' => true]));
        return $callable();
    }

    public static function dynamicClass(): object
    {
        $class = \str_rot13(\array_rand(['fgngvp' => true]));
        return $class::make();
    }
}

// An array naming a class and then self:: or parent:: as its method runs a method
// of that class, with the called class of the code making the call. The classes
// it names are preloaded, so only that called class is missing on another thread.
$namedHelperLine = __LINE__ + 5;
final class AsyncDeclRunTimeNamedChild extends AsyncDeclPreScopeMiddle
{
    public static function helper(): int
    {
        return asyncdecl_run_time_helper(1);
    }

    public static function pool(string $class, string $method): \OxPHP\Shared\Pool
    {
        $rot13 = \str_rot13($method);
        return new \OxPHP\Shared\Pool(factory: static function () use ($class, $rot13): object {
            \set_error_handler(static fn (): bool => true, E_DEPRECATED);
            try {
                return \call_user_func([$class, \str_rot13($rot13)]);
            } finally {
                \restore_error_handler();
            }
        }, destroy: null, maxSize: 1);
    }

    public static function throughArrayMap(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \array_map([AsyncDeclPreScopeBase::class, \str_rot13(\array_rand(['frys::znxr' => true]))], [1])[0];
        } finally {
            \restore_error_handler();
        }
    }

    public static function throughForwardStaticCall(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \forward_static_call([AsyncDeclPreScopeBase::class, \str_rot13(\array_rand(['frys::znxr' => true]))]);
        } finally {
            \restore_error_handler();
        }
    }

    // filter_var() calls the callable by name, and PHP reports this form as valid,
    // so its frame is pushed with the Error pending.
    public static function throughFilterVar(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \filter_var('x', FILTER_CALLBACK, ['options' => [
                AsyncDeclPreScopeBase::class,
                \str_rot13(\array_rand(['frys::znxr' => true])),
            ]]);
        } finally {
            \restore_error_handler();
        }
    }

    public static function throughStreamNotifier(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            $context = \stream_context_create(params: ['notification' => [
                AsyncDeclPreScopeBase::class,
                \str_rot13(\array_rand(['frys::znxr' => true])),
            ]]);
            \file_get_contents('http://127.0.0.1:9090/health', false, $context);
            return new \stdClass();
        } finally {
            \restore_error_handler();
        }
    }

    // The frame pushed with the Error pending unwinds before its first statement.
    public static function startedThroughFilterVar(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \filter_var('x', FILTER_CALLBACK, ['options' => [
                AsyncDeclPreScopeBase::class,
                \str_rot13(\array_rand(['frys::znexFgnegrq' => true])),
            ]]);
        } catch (\Error $e) {
            throw new \RuntimeException('started: ' . \var_export(AsyncDeclPreScopeBase::$started, true), 0, $e);
        } finally {
            \restore_error_handler();
        }
    }
}

final class AsyncDeclRunTimeNamedClean extends AsyncDeclPreScopeMiddle
{
    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func([AsyncDeclPreScopeBase::class, \str_rot13(\array_rand(['frys::znxr' => true]))]);
        } finally {
            \restore_error_handler();
        }
    }
}

$refused = asyncdecl_refused('asyncdecl_run_time_helper', __FILE__, $makeLine);
$invalid = [\TypeError::class, 'call_user_func(): Argument #1 ($callback) must be a valid callback, (null)'];
$pools = [
    "'static::make'" => [[AsyncDeclRunTimeMaker::class, 'staticString'], [$invalid, $refused]],
    "'self::make'" => [[AsyncDeclRunTimeMaker::class, 'selfString'], [$invalid, $refused]],
    "'parent::make'" => [[AsyncDeclRunTimeChild::class, 'parentString'], [$invalid, $refused]],
    "['static', 'make']" => [[AsyncDeclRunTimeMaker::class, 'staticArray'], [$invalid, $refused]],
    'array_map()' => [[AsyncDeclRunTimeMaker::class, 'throughArrayMap'], [$refused]],
    'forward_static_call()' => [[AsyncDeclRunTimeMaker::class, 'throughForwardStaticCall'], [$refused]],
    'filter_var()' => [[AsyncDeclRunTimeMaker::class, 'throughFilterVar'], [$refused]],
    'filter_var() calling call_user_func()' => [[AsyncDeclRunTimeMaker::class, 'throughCallUserFuncInFilterVar'], [$refused]],
    'a stream notifier' => [[AsyncDeclRunTimeMaker::class, 'throughStreamNotifier'], [$refused]],
    'a closure bound elsewhere' => [[AsyncDeclRunTimeMaker::class, 'selfOfAClosureBoundElsewhere'], [$invalid, $refused]],
    'array_map() in a closure bound elsewhere' => [[AsyncDeclRunTimeMaker::class, 'throughArrayMapInAClosureBoundElsewhere'], [$refused]],
    'error_reporting(0)' => [[AsyncDeclRunTimeMaker::class, 'underErrorReportingZero'], [$invalid, $refused]],
];
foreach ($pools as $what => [$factory, $thrown]) {
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: in a task", asyncdecl_run_time_thrown_in_task($pool), $thrown);
    $h = $pool->acquire();
    $t->assertSame("$what: on the worker thread the factory runs", $h->get()->v, 1);
    $h->release();
}

// The first statements run: this is decided at the call, which is what tells it
// from the check made before the callback starts. An error handler for the
// deprecation is not called for a call refused this way, and is for one that
// goes ahead.
$map = new \OxPHP\Shared\Map();
$map->set('callable', 'static::make');
$pool = AsyncDeclRunTimeMaker::fromMap($map);
$t->assertSame('a value of a Shared\Map: in a task', asyncdecl_run_time_thrown_in_task($pool), [$invalid, $refused]);
$t->assertSame('the callback ran up to the call', $map->get('reached the call'), true);
$t->assertSame('the handler is not called for a call refused', $map->get('the handler ran'), null);
$h = $pool->acquire();
$t->assertSame('the handler is called on the worker thread the factory runs', $map->get('the handler ran'), true);
$h->release();

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeSubclass::class, 'factory'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('static standing for a subclass: in a task', asyncdecl_run_time_thrown_in_task($pool), [
    $invalid,
    asyncdecl_refused('asyncdecl_run_time_helper', __FILE__, $subclassMakeLine),
]);

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeWhole::class, 'factory'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('another method of the class: in a task', asyncdecl_run_time_thrown_in_task($pool), [
    $invalid,
    [\Error::class, 'Cannot declare anonymous class at ' . __FILE__ . ":$anonymousLine" . ASYNCDECL_TAIL],
]);
$h = $pool->acquire();
$t->assertSame('another method of the class: on the worker thread the factory runs', $h->get()->v, 8);
$h->release();

// What a destroy callback throws is swallowed, so what is seen here is only
// that the acquire and the evict() in the task came back.
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): object => new \stdClass(),
    destroy: [AsyncDeclRunTimeMaker::class, 'destroyString'],
    maxSize: 1,
);
$t->assertSame('a destroy callback: in a task', asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);

$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeClean::class, 'factory'], destroy: null, maxSize: 1);
$t->assertSame('a class with nothing missing runs in a task', asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);

foreach (['sameTextFromUserCode', 'anotherDeprecation'] as $method) {
    $pool = new \OxPHP\Shared\Pool(
        factory: [AsyncDeclRunTimeNotACallable::class, $method],
        destroy: null,
        maxSize: 1,
    );
    $t->assertSame("$method: runs in a task", asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);
}

foreach (['dynamicCall', 'dynamicClass'] as $method) {
    $pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeByName::class, $method], destroy: null, maxSize: 1);
    $t->assertSame("$method: in a task", asyncdecl_acquire_in_task($pool), [\Error::class, 'Class "static" not found']);
}

$namedRefused = asyncdecl_refused('asyncdecl_run_time_helper', __FILE__, $namedHelperLine);
$named = [
    "['Base', 'self::make']" => AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeBase::class, 'self::make'),
    "['Middle', 'parent::make']" => AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeMiddle::class, 'parent::make'),
    "['Base', 'SELF::make']" => AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeBase::class, 'SELF::make'),
    "['Middle', 'Parent::make']" => AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeMiddle::class, 'Parent::make'),
    'that array through array_map()' => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclRunTimeNamedChild::class, 'throughArrayMap'],
        destroy: null,
        maxSize: 1,
    ),
    'that array through forward_static_call()' => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclRunTimeNamedChild::class, 'throughForwardStaticCall'],
        destroy: null,
        maxSize: 1,
    ),
    'that array through filter_var()' => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclRunTimeNamedChild::class, 'throughFilterVar'],
        destroy: null,
        maxSize: 1,
    ),
];
foreach ($named as $what => $pool) {
    $t->assertSame("$what: in a task", asyncdecl_run_time_thrown_in_task($pool), [$namedRefused]);
    $h = $pool->acquire();
    $t->assertSame("$what: the called class is the caller's on the worker thread", $h->get()->v, 2);
    $h->release();
}

// A class named before the :: resolves by that name, so the called class does not
// come from the code making the call, and nothing is missing.
$t->assertSame(
    "['Base', 'Base::make']: runs in a task",
    asyncdecl_acquire_in_task(AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeBase::class, AsyncDeclPreScopeBase::class . '::make')),
    ['returned', \stdClass::class],
);
// static:: stands for the caller's class, which the class named first must extend,
// so PHP itself refuses the method of a parent.
$t->assertSame(
    "['Base', 'static::make']: in a task",
    asyncdecl_run_time_thrown_in_task(AsyncDeclRunTimeNamedChild::pool(AsyncDeclPreScopeBase::class, 'static::make')),
    [[\TypeError::class, 'call_user_func(): Argument #1 ($callback) must be a valid callback, class '
        . AsyncDeclPreScopeBase::class . ' is not a subclass of ' . AsyncDeclRunTimeNamedChild::class]],
);
$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeNamedClean::class, 'factory'], destroy: null, maxSize: 1);
$t->assertSame('that array in a class with nothing missing runs in a task', asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeNamedChild::class, 'startedThroughFilterVar'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('that array through filter_var(): the method does not start', asyncdecl_run_time_thrown_in_task($pool), [
    [\RuntimeException::class, 'started: false'],
    $namedRefused,
]);

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeNamedChild::class, 'throughStreamNotifier'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('that array as a stream notifier: in a task', asyncdecl_run_time_thrown_in_task($pool), [$namedRefused]);

$t->done();
