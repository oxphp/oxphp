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
}
