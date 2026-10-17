<?php

declare(strict_types=1);

// Included by test_pool_named_factory_created_after_task_started after a
// generated file of thousands of functions. Besides the method's own runtime
// cache, the factory uses its class's per-thread slots through the static
// property.
final class AsyncDeclLateMaker
{
    private static int $made = 0;

    public static function make(): object
    {
        $double = static fn (int $x): int => \intdiv($x * 4, 2);
        $o = new \stdClass();
        $o->made = ++self::$made;
        $o->doubled = \array_map($double, [1, 2, 3]);
        return $o;
    }
}
