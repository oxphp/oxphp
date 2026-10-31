<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_held_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// A callable that names its class as self, static or parent reaches that class
// from a value the factory holds as well as from a literal of its code: a
// value it captured, a static variable's initial value, its class's constant
// or static property, a parameter's default value. Each of these has the
// class checked whole. Each case below has a class of its own, which only
// that value makes the factory reach. PHP deprecates these callables, and
// TestCase turns the notice into an exception whatever @ says, so each is
// called with deprecations ignored.
$t = new TestCase('test_pool_factory_reaches_scope_through_a_held_callable', 'asyncdecl');

$makeLine = __LINE__ + 5;
class AsyncDeclHeldMaker
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_held_scope_helper()];
    }
}

class AsyncDeclHeldCapture extends AsyncDeclHeldMaker
{
    public static function pool(): \OxPHP\Shared\Pool
    {
        $cb = 'static::make';
        return new \OxPHP\Shared\Pool(
            factory: static function () use ($cb): object {
                \set_error_handler(static fn (): bool => true, E_DEPRECATED);
                try {
                    return \call_user_func($cb);
                } finally {
                    \restore_error_handler();
                }
            },
            destroy: null,
            maxSize: 1,
        );
    }
}

class AsyncDeclHeldCaptureByRef extends AsyncDeclHeldMaker
{
    public static function pool(): \OxPHP\Shared\Pool
    {
        $cb = 'static::make';
        return new \OxPHP\Shared\Pool(
            factory: static function () use (&$cb): object {
                \set_error_handler(static fn (): bool => true, E_DEPRECATED);
                try {
                    return \call_user_func($cb);
                } finally {
                    \restore_error_handler();
                }
            },
            destroy: null,
            maxSize: 1,
        );
    }
}

class AsyncDeclHeldClosureStatic extends AsyncDeclHeldMaker
{
    public static function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static function (): object {
                static $cb = 'static::make';
                \set_error_handler(static fn (): bool => true, E_DEPRECATED);
                try {
                    return \call_user_func($cb);
                } finally {
                    \restore_error_handler();
                }
            },
            destroy: null,
            maxSize: 1,
        );
    }
}

class AsyncDeclHeldMethodStatic extends AsyncDeclHeldMaker
{
    public static function factory(): object
    {
        static $cb = 'static::make';
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func($cb);
        } finally {
            \restore_error_handler();
        }
    }
}

class AsyncDeclHeldNestedStatic extends AsyncDeclHeldMaker
{
    public static function factory(): object
    {
        $f = static function (): object {
            static $cb = ['self', 'make'];
            \set_error_handler(static fn (): bool => true, E_DEPRECATED);
            try {
                return \call_user_func($cb);
            } finally {
                \restore_error_handler();
            }
        };
        return $f();
    }
}

// static:: is not resolved when the file is compiled, so the optimizer cannot
// fold the constant or the property into a literal of the method.
class AsyncDeclHeldConstant extends AsyncDeclHeldMaker
{
    public const CB = 'static::make';

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(static::CB);
        } finally {
            \restore_error_handler();
        }
    }
}

interface AsyncDeclHeldIface
{
    public const CB = 'static::make';
}

class AsyncDeclHeldInherited extends AsyncDeclHeldMaker implements AsyncDeclHeldIface
{
    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(static::CB);
        } finally {
            \restore_error_handler();
        }
    }
}

class AsyncDeclHeldStaticProperty extends AsyncDeclHeldMaker
{
    public static string $cb = 'static::make';

    public static function factory(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(static::$cb);
        } finally {
            \restore_error_handler();
        }
    }
}

class AsyncDeclHeldDefault extends AsyncDeclHeldMaker
{
    public const M = 'make';

    // In a closure self:: is not resolved when the file is compiled, so the
    // default value stays a constant expression rather than an array literal.
    public static function pool(): \OxPHP\Shared\Pool
    {
        return new \OxPHP\Shared\Pool(
            factory: static function (array $cb = ['static', self::M]): object {
                \set_error_handler(static fn (): bool => true, E_DEPRECATED);
                try {
                    return \call_user_func($cb);
                } finally {
                    \restore_error_handler();
                }
            },
            destroy: null,
            maxSize: 1,
        );
    }
}

// The same places holding a callable string that names no class.
class AsyncDeclHeldPlain extends AsyncDeclHeldMaker
{
    public const FN = 'strlen';

    public static string $fn = 'strlen';

    public static function capturePool(): \OxPHP\Shared\Pool
    {
        $fn = 'strlen';
        return new \OxPHP\Shared\Pool(
            factory: static fn (): object => (object) ['v' => $fn('a')],
            destroy: null,
            maxSize: 1,
        );
    }

    public static function factory(): object
    {
        static $fn = 'strlen';
        $const = static::FN;
        $prop = static::$fn;
        return (object) ['v' => $fn('a') * $const('a') * $prop('a')];
    }
}

$refused = asyncdecl_refused('asyncdecl_held_scope_helper', __FILE__, $makeLine);
$pools = [
    'a captured value' => AsyncDeclHeldCapture::pool(),
    'a value captured by reference' => AsyncDeclHeldCaptureByRef::pool(),
    "a closure's static variable" => AsyncDeclHeldClosureStatic::pool(),
    "a method's static variable" => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldMethodStatic::class, 'factory'], destroy: null, maxSize: 1),
    "a nested closure's static variable" => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldNestedStatic::class, 'factory'], destroy: null, maxSize: 1),
    'a constant read through static::' => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldConstant::class, 'factory'], destroy: null, maxSize: 1),
    "an interface's constant read through static::" => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldInherited::class, 'factory'], destroy: null, maxSize: 1),
    "a static property's default read through static::" => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldStaticProperty::class, 'factory'], destroy: null, maxSize: 1),
    "a parameter's default value" => AsyncDeclHeldDefault::pool(),
];
foreach ($pools as $what => $pool) {
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool), $refused);
}

$plain = [
    "'strlen' as a captured value" => AsyncDeclHeldPlain::capturePool(),
    "'strlen' as a static variable, a constant and a static property" => new \OxPHP\Shared\Pool(
        factory: [AsyncDeclHeldPlain::class, 'factory'], destroy: null, maxSize: 1),
];
foreach ($plain as $what => $pool) {
    $t->assertSame("$what: runs in the task", asyncdecl_acquire_in_task($pool), ['returned', \stdClass::class]);
}

foreach ($pools + $plain as $what => $pool) {
    $h = $pool->acquire();
    $t->assertSame("$what: on the worker thread the factory runs", $h->get()->v, 1);
    $h->release();
}

$t->done();
