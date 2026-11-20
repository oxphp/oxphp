<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_run_time_helper(int $base = 0): int
{
    return $base + 1;
}

// A callable naming its class as self, static or parent, built while the code
// runs, cannot be found by reading the callback before it is called. It is
// checked when PHP resolves it, or, for a FILTER_CALLBACK on PHP 8.6, when the
// filter_*() function is called: on a thread that lacks that class, the class is
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

    // filter_var() checks the callable with deprecations suppressed. On 8.4 and 8.5
    // PHP resolves it again as filter_var() calls it, and refuses it there; on 8.6
    // filter_var() calls what it resolved, and is refused before it runs.
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

    // A catch in the frame making the call gets the refusal: the VM jumps to it
    // with the Error still pending.
    public static function caughtWhereItIsCalled(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            $made = \call_user_func(\str_rot13(\array_rand(['frys::znxr' => true])));
        } catch (\Error $e) {
            $made = $e;
        }
        \restore_error_handler();
        if ($made instanceof \Error) {
            throw new \RuntimeException('caught where it is called', 0, $made);
        }
        return $made;
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

    // A pool whose factory calls $method of the base class, which starts inside a
    // try, through filter_var(), and throws how many of the base class's catch and
    // finally bodies ran. The factory captures the name ROT13-encoded: a captured
    // value naming self:: would have the check before the callback starts refuse it.
    public static function unwoundPool(string $method): \OxPHP\Shared\Pool
    {
        $rot13 = \str_rot13($method);
        return new \OxPHP\Shared\Pool(factory: static function () use ($rot13): object {
            \set_error_handler(static fn (): bool => true, E_DEPRECATED);
            $unwound = AsyncDeclPreScopeBase::$unwound;
            try {
                return \filter_var('x', FILTER_CALLBACK, ['options' => [AsyncDeclPreScopeBase::class, \str_rot13($rot13)]]);
            } catch (\Error $e) {
                throw new \RuntimeException('unwound: ' . (AsyncDeclPreScopeBase::$unwound - $unwound), 0, $e);
            } finally {
                \restore_error_handler();
            }
        }, destroy: null, maxSize: 1);
    }

    // ob_start() keeps the callable it resolved, the called class of this code
    // with it, and calls it as the buffer is discarded without resolving it again.
    public static function throughObStart(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            try {
                \ob_start([AsyncDeclPreScopeBase::class, \str_rot13(\array_rand(['frys::znxr' => true]))]);
            } catch (\Error) {
            }
            \ob_end_clean();
            return new \stdClass();
        } finally {
            \restore_error_handler();
        }
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

    // On 8.4 and 8.5 filter_var() calls the callable by name, and PHP reports this
    // form as valid, so its frame is pushed with the Error pending.
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

    // The method's first opcode builds a string: on 8.4 and 8.5, unwinding from
    // that opcode, not before it, would release one not written yet.
    public static function quotedThroughFilterVar(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \filter_var('x', FILTER_CALLBACK, ['options' => [
                AsyncDeclPreScopeBase::class,
                \str_rot13(\array_rand(['frys::dhbgr' => true])),
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

    public static function caughtWhereItIsCalled(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            $made = \call_user_func([AsyncDeclPreScopeBase::class, \str_rot13(\array_rand(['frys::znxr' => true]))]);
        } catch (\Error $e) {
            $made = $e;
        }
        \restore_error_handler();
        if ($made instanceof \Error) {
            throw new \RuntimeException('caught where it is called', 0, $made);
        }
        return $made;
    }

    // On 8.4 and 8.5 the frame pushed with the Error pending unwinds before its
    // first statement; on 8.6 no frame is pushed.
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

    // A pool whose factory hands the callable $parts make up, a string or an array,
    // to $function as a FILTER_CALLBACK, and throws whether the method started. The
    // parts are captured ROT13-encoded, as unwoundPool() captures its name.
    public static function startedPool(string $function, string ...$parts): \OxPHP\Shared\Pool
    {
        $rot13 = \array_map(\str_rot13(...), $parts);
        return new \OxPHP\Shared\Pool(factory: static function () use ($function, $rot13): object {
            $parts = \array_map(\str_rot13(...), $rot13);
            $callable = \count($parts) === 1 ? $parts[0] : $parts;
            $spec = ['filter' => FILTER_CALLBACK, 'options' => $callable];
            // A callback that runs first and writes, through references, what PHP
            // reads after it: $callable, a part of it, FILTER_CALLBACK as the id.
            $late = $method = null;
            $id = FILTER_DEFAULT;
            $first = static function (string $v) use (&$late, &$method, &$id, $callable, $parts): string {
                [$late, $method, $id] = [$callable, $parts[1] ?? null, FILTER_CALLBACK];
                return $v;
            };
            $late = $first;
            // The flag is this thread's: a call that started here before left it set.
            AsyncDeclPreScopeBase::$started = false;
            // E_WARNING: the reference case below, a filter id PHP does not know.
            \set_error_handler(static fn (): bool => true, E_DEPRECATED | E_WARNING);
            try {
                match ($function) {
                    'filter_var' => \filter_var('x', FILTER_CALLBACK, ['options' => $callable]),
                    'filter_var, another filter' => \filter_var('x', FILTER_VALIDATE_INT, ['options' => $callable]),
                    'filter_var, the spec naming another filter' => \filter_var('x', FILTER_CALLBACK, ['filter' => FILTER_DEFAULT, 'options' => $callable]),
                    'filter_var, by reference' => \filter_var('x', FILTER_CALLBACK, ['options' => &$callable]),
                    'filter_var, an object' => \filter_var('x', FILTER_CALLBACK, ['options' => [new $parts[0](), $parts[1]]]),
                    'filter_input' => \filter_input(INPUT_ENV, 'PATH', FILTER_CALLBACK, ['options' => $callable]),
                    'filter_var_array' => \filter_var_array(['x' => 'x'], ['x' => $spec]),
                    // PHP converts the id: '1024' is FILTER_CALLBACK.
                    'filter_var_array, the id a string' => \filter_var_array(['x' => 'x'], ['x' => ['filter' => (string) FILTER_CALLBACK] + $spec]),
                    'filter_input_array' => \filter_input_array(INPUT_ENV, ['PATH' => $spec]),
                    // PHP takes an entry that is not an array as a filter id.
                    'filter_var_array, a reference to the spec' => \filter_var_array(['x' => 'x'], ['x' => &$spec]),
                    'filter_var, the options rewritten' => \filter_var(['x', 'x'], FILTER_CALLBACK, ['options' => &$late]),
                    'filter_var_array, a part of the options rewritten' => \filter_var_array(['x' => 'x', 'y' => 'y'], [
                        'x' => ['filter' => FILTER_CALLBACK, 'options' => $first],
                        'y' => ['filter' => FILTER_CALLBACK, 'options' => [$parts[0], &$method]],
                    ]),
                    'filter_var_array, the id rewritten' => \filter_var_array(['x' => 'x', 'y' => 'y'], [
                        'x' => ['filter' => FILTER_CALLBACK, 'options' => $first],
                        'y' => ['filter' => &$id, 'options' => $callable],
                    ]),
                };
            } catch (\Error $e) {
                throw new \RuntimeException('started: ' . \var_export(AsyncDeclPreScopeBase::$started, true), 0, $e);
            } finally {
                \restore_error_handler();
            }
            throw new \RuntimeException('started: ' . \var_export(AsyncDeclPreScopeBase::$started, true));
        }, destroy: null, maxSize: 1);
    }
}

// A method the named class lacks is called through its __callStatic(). On 8.4
// and 8.5 filter_var() pushes the frame that would call it; that frame unwinds
// before __callStatic() starts, and what it holds is released. On 8.6 filter_var()
// is refused before it runs. Either way memory stays flat over repeated refusals.
final class AsyncDeclRunTimeMagicChild extends AsyncDeclPreScopeMagic
{
    public static function helper(): int
    {
        return asyncdecl_run_time_helper(1);
    }

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        $refused = 0;
        $before = 0;
        for ($i = 0; $i <= 100; $i++) {
            if ($i === 1) {
                $before = \memory_get_usage();
            }
            try {
                \filter_var('x', FILTER_CALLBACK, ['options' => [
                    AsyncDeclPreScopeMagic::class,
                    \str_rot13(\array_rand(['frys::nofrag' => true])) . $i,
                ]]);
            } catch (\Error) {
                $refused++;
            }
        }
        // Read before `new`, which builds the exception and its trace before its arguments.
        $grew = \memory_get_usage() - $before;
        \restore_error_handler();
        throw new \RuntimeException(\sprintf(
            'refused %d, __callStatic() ran %d, grew %d',
            $refused,
            AsyncDeclPreScopeMagic::$calls,
            $grew,
        ));
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
    'a catch around the call' => [
        [AsyncDeclRunTimeMaker::class, 'caughtWhereItIsCalled'],
        [[\RuntimeException::class, 'caught where it is called'], $invalid, $refused],
    ],
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

// The other forms PHP deprecates, and the other filter_*() functions taking a
// FILTER_CALLBACK. filter_input() and filter_input_array() read PATH from the
// environment, there on every thread, so the callback has input to run on.
foreach ([
    "'Parent::markStarted' through filter_var()" => ['filter_var', 'Parent::markStarted'],
    "['parent', 'markStarted'] through filter_var()" => ['filter_var', 'parent', 'markStarted'],
    "['Base', 'self::markStarted'] by reference through filter_var()" => ['filter_var, by reference', AsyncDeclPreScopeBase::class, 'self::markStarted'],
    "[new Base, 'self::markStarted'] through filter_var()" => ['filter_var, an object', AsyncDeclPreScopeBase::class, 'self::markStarted'],
    "['Base', 'self::markStarted'] through filter_input()" => ['filter_input', AsyncDeclPreScopeBase::class, 'self::markStarted'],
    "['Base', 'self::markStarted'] through filter_var_array()" => ['filter_var_array', AsyncDeclPreScopeBase::class, 'self::markStarted'],
    "['Base', 'self::markStarted'] through filter_var_array(), the id a string" => ['filter_var_array, the id a string', AsyncDeclPreScopeBase::class, 'self::markStarted'],
    "['Base', 'self::markStarted'] through filter_input_array()" => ['filter_input_array', AsyncDeclPreScopeBase::class, 'self::markStarted'],
] as $what => $args) {
    $t->assertSame("$what: the method does not start", asyncdecl_run_time_thrown_in_task(AsyncDeclRunTimeNamedChild::startedPool(...$args)), [
        [\RuntimeException::class, 'started: false'],
        $namedRefused,
    ]);
}
// Each call starts with no FILTER_CALLBACK of these forms, and a callback that runs
// first writes, through a reference, what PHP reads next: the callable into the
// options or a part of them, or FILTER_CALLBACK into the id of an entry whose
// options are the callable.
foreach ([
    'written into the options' => 'filter_var, the options rewritten',
    'written into a part of the options' => 'filter_var_array, a part of the options rewritten',
    'selected by FILTER_CALLBACK written into the filter id' => 'filter_var_array, the id rewritten',
] as $what => $function) {
    $t->assertSame(
        "['Base', 'self::markStarted'] $what by a callback before it: the method does not start",
        asyncdecl_run_time_thrown_in_task(AsyncDeclRunTimeNamedChild::startedPool($function, AsyncDeclPreScopeBase::class, 'self::markStarted')),
        [[\RuntimeException::class, 'started: false'], $namedRefused],
    );
}
// No FILTER_CALLBACK runs here, so nothing is refused.
foreach ([
    'with another filter' => 'filter_var, another filter',
    'with a spec naming another filter' => 'filter_var, the spec naming another filter',
    'through filter_var_array(), a reference to the spec' => 'filter_var_array, a reference to the spec',
] as $what => $function) {
    $t->assertSame(
        "['Base', 'self::markStarted'] $what: in a task",
        asyncdecl_run_time_thrown_in_task(AsyncDeclRunTimeNamedChild::startedPool($function, AsyncDeclPreScopeBase::class, 'self::markStarted')),
        [[\RuntimeException::class, 'started: false']],
    );
}
// static:: in the method stands for the caller's class, which the class named first
// must extend, so PHP refuses this one itself, as it does through call_user_func().
$t->assertSame(
    "['Base', 'static::markStarted'] through filter_var(): in a task",
    asyncdecl_run_time_thrown_in_task(AsyncDeclRunTimeNamedChild::startedPool('filter_var', AsyncDeclPreScopeBase::class, 'static::markStarted')),
    [[\RuntimeException::class, 'started: false'], [\TypeError::class, 'filter_var(): Option must be a valid callback']],
);

// On 8.4 and 8.5, unwinding before the method's first opcode passes its own try,
// so neither its catch nor its finally runs on the thread that lacks the called
// class. On 8.6 the method is not reached at all.
foreach (['self::tryMake' => 'a catch', 'self::finallyMake' => 'a finally'] as $method => $what) {
    $pool = AsyncDeclRunTimeNamedChild::unwoundPool($method);
    $t->assertSame("that array through filter_var(), the method starting in a try with $what: in a task", asyncdecl_run_time_thrown_in_task($pool), [
        [\RuntimeException::class, 'unwound: 0'],
        $namedRefused,
    ]);
    $h = $pool->acquire();
    $t->assertSame("that array through filter_var(), the method starting in a try with $what: on the worker thread the factory runs", $h->get()->v, 2);
    $h->release();
}

// The method ob_start() calls later starts; the method of the called class it
// calls in turn is refused as it is entered.
$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeNamedChild::class, 'throughObStart'], destroy: null, maxSize: 1);
$t->assertSame('that array kept by ob_start(): in a task', asyncdecl_run_time_thrown_in_task($pool), [$namedRefused]);

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeNamedChild::class, 'quotedThroughFilterVar'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('that array through filter_var(), the method starting with a string: in a task', asyncdecl_run_time_thrown_in_task($pool), [$namedRefused]);
$h = $pool->acquire();
$t->assertSame('that array through filter_var(), the method starting with a string: on the worker thread the factory runs', $h->get()->v, '<x>');
$h->release();

$pool = new \OxPHP\Shared\Pool(
    factory: [AsyncDeclRunTimeNamedChild::class, 'throughStreamNotifier'],
    destroy: null,
    maxSize: 1,
);
$t->assertSame('that array as a stream notifier: in a task', asyncdecl_run_time_thrown_in_task($pool), [$namedRefused]);

$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeNamedChild::class, 'caughtWhereItIsCalled'], destroy: null, maxSize: 1);
$t->assertSame('that array, with a catch around the call: in a task', asyncdecl_run_time_thrown_in_task($pool), [
    [\RuntimeException::class, 'caught where it is called'],
    $namedRefused,
]);

$pool = new \OxPHP\Shared\Pool(factory: [AsyncDeclRunTimeMagicChild::class, 'factory'], destroy: null, maxSize: 1);
$t->assertSame('that array reaching __callStatic() through filter_var(): in a task', asyncdecl_run_time_thrown_in_task($pool), [
    [\RuntimeException::class, 'refused 101, __callStatic() ran 0, grew 0'],
]);

$t->done();
