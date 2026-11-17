<?php

declare(strict_types=1);

// Shared by the tests that the engine's guard against autoloading a class from
// inside its own autoloader belongs to the request that is loading it: it holds
// in every fiber of that request, a fatal in an autoloader does not leave the
// class marked as being loaded for the rest of the worker's life, and a request
// parked inside an autoloader does not stop another request on the worker from
// loading the same class.
//
// Pulled in with require_once for the reason fiber_park_registry.php gives.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/thread_cell_probe.php';

if (!class_exists('OxphpAutoloadGuard', false)) {
    /**
     * An autoloader that acts only on the class names a test arms, and does
     * what the test armed for the first lookup that reaches it. Static, so
     * every request on the worker sees the same record.
     */
    final class OxphpAutoloadGuard
    {
        /**
         * Per armed class, lowercased: what the next lookup does, and how many
         * lookups reached the loader.
         *
         * @var array<string, array{mode: string, calls: int}>
         */
        public static array $classes = [];

        /**
         * Edges of a hold, as "<class>:in", "<class>:out" and "<class>:release";
         * and what a lookup of the class it is loading, made from inside the
         * autoloader once a hold is let go or from another fiber the autoloader
         * runs, does, as "<class>:found_inside" and "<class>:reentered", with
         * "<class>:other_fiber" for whether that other fiber was one.
         *
         * @var array<string, bool>
         */
        public static array $flags = [];

        public static function arm(string $class, string $mode): void
        {
            spl_autoload_register([self::class, 'load']);
            self::$classes[strtolower($class)] = ['mode' => $mode, 'calls' => 0];
        }

        public static function calls(string $class): int
        {
            return self::$classes[strtolower($class)]['calls'] ?? 0;
        }

        public static function load(string $class): void
        {
            $key = strtolower($class);
            if (!isset(self::$classes[$key])) {
                return;
            }
            self::$classes[$key]['calls']++;
            $mode = self::$classes[$key]['mode'];
            self::$classes[$key]['mode'] = 'define';

            if ($mode === 'fatal') {
                engine_fatal($class . 'Broken');
            }
            if ($mode === 'hold') {
                self::$flags["$key:in"] = true;
                // Parks this request inside the autoloader until the test lets
                // it go.
                thread_cell_wait_for(self::$flags, "$key:release", 6.0);
                // Unless a lookup made while this one was held has declared
                // the class, the engine answers this one "not found" without
                // reaching the autoloader, as it would before the hold.
                $calls = self::$classes[$key]['calls'];
                self::$flags["$key:found_inside"] = class_exists($class);
                self::$flags["$key:reentered"] = self::$classes[$key]['calls'] !== $calls;
                self::$flags["$key:out"] = true;
            }
            if ($mode === 'fiber' || $mode === 'gc') {
                $calls = self::$classes[$key]['calls'];
                $outer = \Fiber::getCurrent();
                $lookup = static fn (): array => [class_exists($class), \Fiber::getCurrent() !== $outer];
                $seen = [null, null];
                if ($mode === 'fiber') {
                    $inner = new \Fiber($lookup);
                    $inner->start();
                    $seen = $inner->getReturn();
                } else {
                    // A cycle, so only the collector frees it, and runs the
                    // destructor in a fiber of its own while a fiber is running.
                    $garbage = new OxphpAutoloadGuardGarbage(static function () use ($lookup, &$seen): void {
                        $seen = $lookup();
                    });
                    $garbage->self = $garbage;
                    unset($garbage);
                    gc_collect_cycles();
                }
                [$found, $other] = $seen;
                self::$flags["$key:found_inside"] = $found;
                self::$flags["$key:reentered"] = self::$classes[$key]['calls'] !== $calls;
                self::$flags["$key:other_fiber"] = $other;
            }

            // A lookup made while this one was held, or from the other fiber,
            // may have declared it. Only armed names get here, and the fixtures
            // arm names made of letters and digits only.
            if (!class_exists($class, false)) {
                eval("final class $class {}");
            }
        }
    }

    /** Calls back from its destructor. */
    final class OxphpAutoloadGuardGarbage
    {
        public ?self $self = null;

        public function __construct(private readonly \Closure $onDestruct)
        {
        }

        public function __destruct()
        {
            ($this->onDestruct)();
        }
    }
}
