<?php

declare(strict_types=1);

// Loaded only on PHP 8.5, the first version that accepts a closure in a
// constant expression: test_pool_factory_reaches_a_closure_in_its_class_data
// itself has to compile on 8.4.

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_class_data_helper(int $base = 0): int
{
    return $base + 1;
}

/** The line of each call to asyncdecl_class_data_helper() below. */
const ASYNCDECL_CLASS_DATA_LINES = [
    'a constant' => __LINE__ + 12,
    'a static property' => __LINE__ + 23,
    'a property' => __LINE__ + 34,
    "a child's static property" => __LINE__ + 55,
    "an interface's constant" => __LINE__ + 64,
];

// Each closure is created from its class's constant expression the first time
// a thread reads the constant or the static property, or creates an object.
class AsyncDeclClassDataConstant
{
    public const MAKE = static function (): object {
        return (object) ['v' => asyncdecl_class_data_helper()];
    };

    public static function make(): object
    {
        return (self::MAKE)();
    }
}

class AsyncDeclClassDataStatic
{
    public static \Closure $make = static function (): object {
        return (object) ['v' => asyncdecl_class_data_helper()];
    };

    public static function make(): object
    {
        return (self::$make)();
    }
}

class AsyncDeclClassDataProperty
{
    public \Closure $make = static function (): object {
        return (object) ['v' => asyncdecl_class_data_helper()];
    };

    public static function make(): object
    {
        return ((new self())->make)();
    }
}

// static:: stands for the class the call names, which need not be the one
// that declares the method.
class AsyncDeclClassDataBase
{
    public static function make(): object
    {
        return (static::$make)();
    }
}

final class AsyncDeclClassDataChild extends AsyncDeclClassDataBase
{
    public static \Closure $make = static function (): object {
        return (object) ['v' => asyncdecl_class_data_helper()];
    };
}

// A constant a class inherits is evaluated with the class or interface that
// declares it as scope: self::HELPER in MAKE reads the interface's HELPER, not
// the one the class overrides it with.
interface AsyncDeclClassDataIface
{
    public const HELPER = static function (): object {
        return (object) ['v' => asyncdecl_class_data_helper()];
    };

    public const MAKE = static function (): object {
        return (self::HELPER)();
    };
}

final class AsyncDeclClassDataImplements implements AsyncDeclClassDataIface
{
    public const HELPER = 1;

    public static function make(): object
    {
        return (self::MAKE)();
    }
}

// The initial values of a closure's static variables are kept with its code,
// in a constant expression as in a body.
const ASYNCDECL_CLASS_DATA_STATIC_LINE = __LINE__ + 5;
class AsyncDeclClassDataStaticMaker
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_class_data_helper()];
    }
}

final class AsyncDeclClassDataStaticInConstant extends AsyncDeclClassDataStaticMaker
{
    public const MAKE = static function (): object {
        static $cb = 'static::make';
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func($cb);
        } finally {
            \restore_error_handler();
        }
    };

    public static function factory(): object
    {
        return (self::MAKE)();
    }
}

final class AsyncDeclClassDataStaticInDefault extends AsyncDeclClassDataStaticMaker
{
    public static function factory(\Closure $make = static function (): object {
        static $cb = 'static::make';
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func($cb);
        } finally {
            \restore_error_handler();
        }
    }): object {
        return $make();
    }
}
