<?php

declare(strict_types=1);

// Included only on PHP 8.5, the first version that accepts a closure and a
// first-class callable in a parameter's default value, by
// test_task_default_value_closure_of_late_file after a generated file of
// thousands of functions.
return static function (
    \Closure $double = static function (int $x): int {
        return \intdiv($x * 4, 2);
    },
    \Closure $len = \strlen(...),
): array {
    return [$double(21), $len('abc')];
};
