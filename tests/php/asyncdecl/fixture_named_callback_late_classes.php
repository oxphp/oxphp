<?php

declare(strict_types=1);

// Included by the tests of Shared\Pool callbacks reaching a class linked in
// request memory, after they declared AsyncDeclLinkedEvalParent and
// AsyncDeclLinkedEvalInterface with eval(). OPcache keeps this file in shared
// memory, but a class whose parent or interface is not there is linked by the
// request that includes it, into a copy of its own, the methods it declares
// included.

class AsyncDeclLinkedMaker extends AsyncDeclLinkedEvalParent
{
    public const LIMIT = 5;

    public static int $n = 0;

    public int $v = 7;

    public static function appended(): object
    {
        static $log = '';
        $log .= str_repeat('x', 3);
        return (object) ['v' => $log];
    }

    public static function counted(): object
    {
        static $n = 0;
        return (object) ['v' => ++$n];
    }

    public static function neverCalledBefore(): object
    {
        static $n = 0;
        return (object) ['v' => ++$n];
    }

    public static function plain(): object
    {
        return (object) ['v' => 1];
    }

    public static function calledClass(): object
    {
        return (object) ['v' => static::class];
    }

    public static function currentClosure(): object
    {
        try {
            \Closure::getCurrent();
            return (object) ['v' => 'a closure'];
        } catch (\Error $e) {
            return (object) ['v' => $e->getMessage()];
        }
    }

    public static function namesATaskFirst(): object
    {
        return new AsyncDeclLinkedTarget();
    }

    public static function namesAThreadFirst(): object
    {
        return new AsyncDeclLinkedTarget();
    }

    public static function namesATaskFirstAsAClosure(): object
    {
        return new AsyncDeclLinkedTarget();
    }

    public static function namesAThreadFirstAsAClosure(): object
    {
        return new AsyncDeclLinkedTarget();
    }

    public static function viaSelf(): object
    {
        return (object) ['v' => self::helper()];
    }

    public static function viaParent(): object
    {
        return parent::make();
    }

    public static function viaNewStatic(): object
    {
        return new static();
    }

    public static function viaSelfProperty(): object
    {
        return (object) ['v' => ++self::$n];
    }

    public static function viaStaticConstant(): object
    {
        return (object) ['v' => static::LIMIT];
    }

    public static function declaredClosure(): \Closure
    {
        return static fn (): object => (object) ['v' => self::helper()];
    }

    /** The number of slots it was given so far, with null only reporting it. */
    public static function destroyCounted(?object $slot): int
    {
        static $n = 0;
        return $slot ? ++$n : $n;
    }

    public static function destroyPlain(object $slot): void
    {
        $slot->destroyed->add(1);
    }

    private static function helper(): int
    {
        return 3;
    }
}

class AsyncDeclLinkedMagic extends AsyncDeclLinkedEvalParent
{
    public static function __callStatic(string $name, array $args): object
    {
        static $n = 0;
        return (object) ['v' => ++$n];
    }
}

class AsyncDeclLinkedShmBase
{
    public static function viaStatic(): object
    {
        return (object) ['v' => static::helper()];
    }

    public static function inheritedCounted(): object
    {
        static $n = 0;
        $n++;
        return (object) ['v' => 'inherited'];
    }

    public static function __callStatic(string $name, array $args): object
    {
        return (object) ['v' => 2];
    }

    protected static function helper(): int
    {
        return 0;
    }
}

final class AsyncDeclLinkedShmChild extends AsyncDeclLinkedShmBase implements AsyncDeclLinkedEvalInterface
{
    protected static function helper(): int
    {
        static $n = 0;
        return ++$n;
    }
}

// A class with neither parent nor interface: OPcache keeps it linked in shared
// memory, so every thread has static properties and a run-time cache of its own
// for it.
class AsyncDeclLinkedInSharedMemory
{
    public const LIMIT = 6;

    public static int $n = 0;

    public int $v = 8;

    public static function viaSelf(): object
    {
        return (object) ['v' => self::helper()];
    }

    public static function viaNewStatic(): object
    {
        return new static();
    }

    public static function viaSelfProperty(): object
    {
        return (object) ['v' => ++self::$n];
    }

    public static function viaStaticConstant(): object
    {
        return (object) ['v' => static::LIMIT];
    }

    public static function __callStatic(string $name, array $args): object
    {
        return (object) ['v' => self::helper()];
    }

    private static function helper(): int
    {
        return 1;
    }
}

final class AsyncDeclLinkedForwarding extends AsyncDeclPreForwarder implements AsyncDeclLinkedEvalInterface
{
    public static function helper(): object
    {
        static $n = 0;
        return (object) ['v' => ++$n];
    }

    public static function forwardsItself(): object
    {
        return forward_static_call([AsyncDeclPreForwarder::class, 'target']);
    }
}

// Nothing it extends or implements is outside shared memory. The enum its method
// returns is: the check of that type against the interface's leaves this class
// to the request that links it.
enum AsyncDeclLinkedVarianceStatus
{
    case On;
}

interface AsyncDeclLinkedHasStatus
{
    public function status(): \UnitEnum;
}

class AsyncDeclLinkedByVariance implements AsyncDeclLinkedHasStatus
{
    public int $v = 11;

    public function status(): AsyncDeclLinkedVarianceStatus
    {
        return AsyncDeclLinkedVarianceStatus::On;
    }

    public static function viaNewStatic(): object
    {
        return new static();
    }
}

final class AsyncDeclLinkedTarget
{
    public int $v = 16;
}

function asyncdecl_linked_anonymous_class(): string
{
    $object = new class extends AsyncDeclLinkedEvalParent {
        public static function viaSelf(): object
        {
            return (object) ['v' => self::helper()];
        }

        private static function helper(): int
        {
            return 3;
        }
    };
    return $object::class;
}
