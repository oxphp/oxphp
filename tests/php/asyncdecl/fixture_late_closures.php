<?php

declare(strict_types=1);

// Included by test_task_nested_closure_of_late_file after a generated file of
// thousands of functions. The task creates three closures, each calling a
// built-in function: an arrow function, one that captures a variable, and one
// with a static variable.
return static function (): array {
    $double = static fn (int $x): int => \intdiv($x * 4, 2);
    $k = 10;
    $add = static function (int $x) use ($k): int {
        return \max($x, 0) + $k;
    };
    $count = static function (): int {
        static $n = 0;
        return \intdiv(++$n * 2, 2);
    };
    return [\array_map($double, [1, 2, 3]), $add(1), $count(), $count()];
};
