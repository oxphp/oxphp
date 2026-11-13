<?php

declare(strict_types=1);

// The objects the argument fixtures pass before they fatal, held by the class
// across requests so that a test can count their references after the fatal.
// Declared conditionally: the worker keeps the class, and a second declaration
// of it would fail.

if (!class_exists('OxphpArgumentProbe', false)) {
    final class OxphpArgumentProbe
    {
        public static ?object $held = null;

        public static ?object $kept = null;

        public static function take(mixed $a): void
        {
        }

        public static function take2(mixed $k, mixed $a): void
        {
        }

        public static function takeRef2(mixed $k, mixed &$a): void
        {
        }

        public static function refcount(): int
        {
            ob_start();
            debug_zval_dump(self::$held);
            return (int) preg_replace('/^.*?refcount\((\d+)\).*$/s', '$1', (string) ob_get_clean());
        }

        public static function keptRefcount(): int
        {
            ob_start();
            debug_zval_dump(self::$kept);
            return (int) preg_replace('/^.*?refcount\((\d+)\).*$/s', '$1', (string) ob_get_clean());
        }
    }

    // Functions, not static methods: the tracing JIT compiles a function call
    // itself, and leaves a static method call to the interpreter.
    function oxphp_argument_outer(mixed $a, mixed $b): void
    {
    }

    function oxphp_argument_inner(mixed $a, mixed $b, mixed $c): object
    {
        return OxphpArgumentProbe::$held;
    }

    // Named, so that registering it again on a later request adds nothing.
    function oxphp_argument_autoload(string $class): void
    {
        if ($class === 'OxphpArgumentUnloaded') {
            engine_fatal('OxphpArgumentWarningFatal');
        }
    }
}
