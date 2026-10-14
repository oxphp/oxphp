<?php

declare(strict_types=1);

// Loaded only on PHP 8.5, the first version that accepts a closure in a
// parameter's default value: test_task_default_value_closure_calls_function
// itself has to compile on 8.4.

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_default_helper(int $base = 0): int
{
    return $base + 31;
}

/** The line of each call to asyncdecl_default_helper() below. */
const ASYNCDECL_DEFAULT_CALL_LINES = [__LINE__ + 8, __LINE__ + 16];

// Tasks whose bodies only call what their parameter defaults to: the closure
// itself, and a closure inside an array. The last one's closure does only
// built-in work.
return [
    static function (\OxPHP\Shared\Counter $ran): \Closure {
        return static function (\Closure $inner = static function (): int {
            return asyncdecl_default_helper();
        }) use ($ran): int {
            $ran->add();
            return $inner();
        };
    },
    static function (\OxPHP\Shared\Counter $ran): \Closure {
        return static function (array $inner = [static function (): int {
            return asyncdecl_default_helper();
        }]) use ($ran): int {
            $ran->add();
            return $inner[0]();
        };
    },
    static function (\OxPHP\Shared\Counter $ran): \Closure {
        return static function (\Closure $inner = static function (): int {
            return \random_int(5, 5);
        }) use ($ran): int {
            $ran->add();
            return $inner();
        };
    },
];
