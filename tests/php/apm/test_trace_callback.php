<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('trace_callback', 'apm');

// Everything runs inside an outer span so that "the span id before the call" is
// a real 16-hex id rather than the empty string. Without it, "the id is restored
// after the call" would be satisfied by a build that never opens a span at all
// ('' === ''), and the assertion would prove nothing.
$outerId = oxphp_apm_start('trace_callback.outer');
$outer = oxphp_apm_span_id();
$t->assertMatch('an outer span is open before the call', $outer, '/^[0-9a-f]{16}$/');

// ── The callback runs, and it runs inside a span of its own ──

$called = false;
$spanArg = null;
$inside = null;

$result = oxphp_apm_trace(
    'trace_callback.child',
    function (int $span) use (&$called, &$spanArg, &$inside): string {
        $called = true;
        $spanArg = $span;
        $inside = oxphp_apm_span_id();
        return 'callback-return';
    },
    ['component' => 'trace-callback-test']
);

$t->assertTrue('the callback is invoked', $called);
$t->assertSame('the callback return value is forwarded', $result, 'callback-return');
$t->assertType('the callback receives an int span id', $spanArg, 'integer');
$t->assertNotEqual('the span id argument is a real span, not 0', $spanArg, 0);

// The span is open *around* the callback: a fresh id, and not the outer one.
$t->assertMatch('a span is open inside the callback', (string) $inside, '/^[0-9a-f]{16}$/');
$t->assertNotEqual('the span inside the callback is not the outer span', $inside, $outer);

// ...and it is closed once the callback returns.
$t->assertSame('the span is closed when the callback returns', oxphp_apm_span_id(), $outer);

// ── Throw path: the exception propagates and the span still closes ──

$insideThrow = null;
$thrown = null;
try {
    oxphp_apm_trace('trace_callback.throws', function (int $span) use (&$insideThrow): void {
        $insideThrow = oxphp_apm_span_id();
        throw new RuntimeException('trace callback boom');
    });
} catch (\Throwable $e) {
    $thrown = $e;
}

$t->assertNotNull('the callback exception propagates out of oxphp_apm_trace', $thrown);
$t->assertSame(
    'the propagated exception is the one the callback threw',
    $thrown?->getMessage(),
    'trace callback boom'
);
// Read the premise at the moment it mattered: a span really was open while the
// callback ran, so "restored afterwards" below is about closing that span rather
// than about no span ever having existed.
$t->assertMatch('a span was open inside the throwing callback', (string) $insideThrow, '/^[0-9a-f]{16}$/');
$t->assertNotEqual('the throwing callback ran under its own span', $insideThrow, $outer);
$t->assertSame('the span is closed on the throw path too', oxphp_apm_span_id(), $outer);

// ── Every callable form works, not just closures ──
//
// The invocation goes through the engine's own callable resolution rather than
// through call_user_func(), so the forms below have to work without any of them
// being special-cased here. A closure-only implementation passes everything
// above this line.

class TraceCbTarget
{
    public function method(int $span): string
    {
        return 'from-method';
    }

    public static function statically(int $span): string
    {
        return 'from-static';
    }

    public function __invoke(int $span): string
    {
        return 'from-invoke';
    }
}

class TraceCbMagic
{
    public function __call(string $name, array $args): string
    {
        return 'from-__call';
    }
}

$target = new TraceCbTarget();

$t->assertSame(
    'an [object, method] array callable runs',
    oxphp_apm_trace('trace_callback.form.array', [$target, 'method']),
    'from-method'
);
$t->assertSame(
    'a "Class::method" string callable runs',
    oxphp_apm_trace('trace_callback.form.string', 'TraceCbTarget::statically'),
    'from-static'
);
$t->assertSame(
    'an invokable object runs',
    oxphp_apm_trace('trace_callback.form.invokable', $target),
    'from-invoke'
);
$t->assertSame(
    'a first-class callable runs',
    oxphp_apm_trace('trace_callback.form.fcc', $target->method(...)),
    'from-method'
);
// A __call trampoline is the form that allocates an engine-side function
// handler the call has to consume; a leak here would not be visible from PHP,
// but a mishandled trampoline crashes rather than returning.
$t->assertSame(
    'a __call trampoline runs',
    oxphp_apm_trace('trace_callback.form.trampoline', [new TraceCbMagic(), 'anything']),
    'from-__call'
);
$t->assertSame('every callable form left the span stack where it was', oxphp_apm_span_id(), $outer);

// ── A value that is not callable is refused, and the error names this function ──

$typeError = null;
try {
    /** @phpstan-ignore-next-line — deliberately not a callable */
    oxphp_apm_trace('trace_callback.bad', 'oxphp_no_such_function_zzz');
} catch (\TypeError $e) {
    $typeError = $e;
}

$t->assertNotNull('a non-callable second argument throws TypeError', $typeError);
$t->assertContains(
    'the TypeError names oxphp_apm_trace, not the function used to invoke',
    $typeError?->getMessage() ?? '',
    'oxphp_apm_trace()'
);
$t->assertContains(
    'the TypeError names the callback argument position',
    $typeError?->getMessage() ?? '',
    'Argument #2'
);
$t->assertSame('a refused callable leaves no span open', oxphp_apm_span_id(), $outer);

// ── An exception raised while *resolving* the callable is not masked ──
//
// Resolving a "Cls::method" string goes through the autoloader, which is free
// to throw. That is not "the second argument is not callable": reporting it as
// such makes this function raise its own TypeError on top of the live
// exception, leaving the real cause reachable only via getPrevious().

$autoloader = static function (string $class): void {
    throw new LogicException("autoload refused $class");
};
spl_autoload_register($autoloader);
$fromAutoload = null;
try {
    /** @phpstan-ignore-next-line — the class is deliberately absent */
    oxphp_apm_trace('trace_callback.autoload', 'TraceCbNeverDefined::nope');
} catch (\Throwable $e) {
    $fromAutoload = $e;
}
spl_autoload_unregister($autoloader);

$t->assertNotNull('the autoloader exception propagates out of oxphp_apm_trace', $fromAutoload);
$t->assertTrue(
    'it propagates as itself, not as a TypeError about the argument',
    $fromAutoload instanceof \LogicException
);
$t->assertContains(
    'the propagated message is the autoloader\'s',
    $fromAutoload?->getMessage() ?? '',
    'autoload refused'
);
$t->assertSame('a resolve-time exception leaves no span open', oxphp_apm_span_id(), $outer);

// ── A deprecation turned into an exception during resolution, likewise ──
//
// "self::m" resolves *successfully* while emitting E_DEPRECATED, so an error
// handler that throws leaves an exception pending on a callable the engine then
// declines to invoke. The callback not running is PHP's own semantics; what
// must not happen is our TypeError landing on top of the handler's exception.

class TraceCbScope
{
    public static function helper(int $span): string
    {
        return 'from-self';
    }

    public function traceViaSelf(): mixed
    {
        return oxphp_apm_trace('trace_callback.form.self', 'self::helper');
    }
}

set_error_handler(static function (int $no, string $msg): bool {
    throw new ErrorException($msg, 0, $no);
});
$fromDeprecation = null;
try {
    (new TraceCbScope())->traceViaSelf();
} catch (\Throwable $e) {
    $fromDeprecation = $e;
}
restore_error_handler();

$t->assertNotNull('a throwing error handler on the deprecation propagates', $fromDeprecation);
$t->assertTrue(
    'it propagates as the handler raised it, not as a TypeError',
    $fromDeprecation instanceof \ErrorException
);
$t->assertSame('the deprecation path leaves no span open', oxphp_apm_span_id(), $outer);

// ── A by-reference callable is a smoke case, not a discriminating one ──
//
// The shim unwraps an IS_REFERENCE return so an internal function never hands a
// reference back to userland (call_user_func unwraps for the same reason). PHP
// code cannot read the zval type, and assignment would dereference either way,
// so this only proves the form runs and the value survives.

$refTarget = 'ref-value';
$byRef = function &(int $span) use (&$refTarget): string {
    return $refTarget;
};
$t->assertSame(
    'a by-reference callable returns its value',
    oxphp_apm_trace('trace_callback.form.byref', $byRef),
    'ref-value'
);
$t->assertSame('the by-reference form left the span stack where it was', oxphp_apm_span_id(), $outer);

oxphp_apm_end($outerId);

$t->done();
