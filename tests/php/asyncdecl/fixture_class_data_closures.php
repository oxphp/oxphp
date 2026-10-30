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
    'constant' => __LINE__ + 11,
    'static property' => __LINE__ + 22,
    'property' => __LINE__ + 33,
    "a child's static property" => __LINE__ + 54,
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
