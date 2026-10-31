<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

/**
 * Await a task: [true, its value] when it returned, [false, the message] when
 * it failed.
 *
 * @return array{bool, mixed}
 */
function asyncdecl_settle(int $promise): array
{
    try {
        return [true, oxphp_async_await($promise)];
    } catch (\OxPHP\Async\AsyncException $e) {
        return [false, $e->getMessage()];
    }
}

/** The tail of every message the check throws. */
const ASYNCDECL_TAIL = ': it belongs to a script this PHP thread has not loaded; only built-in '
    . 'functions and classes and those loaded by opcache.preload exist on every PHP thread, '
    . 'so the code was not run';

/**
 * acquire() a slot of $pool in a task, release it and evict it there: ['returned',
 * the slot's class] when the factory ran, [the class, the message] of what
 * acquire() threw when it did not.
 *
 * @return array{string, string}
 */
function asyncdecl_acquire_in_task(\OxPHP\Shared\Pool $pool): array
{
    [$ok, $result] = asyncdecl_settle(oxphp_async(static function () use ($pool): array {
        try {
            $h = $pool->acquire();
        } catch (\Throwable $e) {
            return [$e::class, $e->getMessage()];
        }
        $class = $h->get()::class;
        $h->release();
        $pool->evict();
        return ['returned', $class];
    }));
    return $ok ? $result : ['the task failed', $result];
}

/**
 * What acquire() returns, by asyncdecl_acquire_in_task(), when the check refuses
 * a call to $function at $file:$line.
 *
 * @return array{string, string}
 */
function asyncdecl_refused(string $function, string $file, int $line): array
{
    return [\Error::class, "Call to undefined function $function() at $file:$line" . ASYNCDECL_TAIL];
}

/** The tail of the message for a variable of the closure that is not copied. */
const ASYNCDECL_COPY_TAIL = ': on a thread other than the one that created the pool, a Shared\Pool '
    . "callback runs with copies of its closure's variables, and only null, booleans, numbers, "
    . 'strings, arrays of these and OxPHP\Shared objects are copied, so the code was not run';

/** The tail of the message for a callback that uses $this. */
const ASYNCDECL_THIS_TAIL = ' uses $this: on a thread other than the one that created the pool, a '
    . 'Shared\Pool callback cannot use an object of the request that created it, so the code was not run';

/** The tail of the message for a method called on an object that does not use $this. */
const ASYNCDECL_ON_OBJECT_TAIL = ' is called on an object: on a thread other than the one that created '
    . 'the pool, a Shared\Pool callback cannot use an object of the request that created it, so the code '
    . 'was not run';

/**
 * What acquire() returns, by asyncdecl_acquire_in_task(), when variable
 * $variable of the closure at $file:$line holds $what.
 *
 * @return array{string, string}
 */
function asyncdecl_not_copied(string $variable, string $what, string $file, int $line): array
{
    return [\Error::class, "Variable \$$variable of the closure at $file:$line holds $what" . ASYNCDECL_COPY_TAIL];
}

/**
 * What acquire() returns, by asyncdecl_acquire_in_task(), when $what — "The
 * closure at file:line", "Method Class::method()" or "Built-in method
 * Class::method()" — uses $this.
 *
 * @return array{string, string}
 */
function asyncdecl_uses_this(string $what): array
{
    return [\Error::class, $what . ASYNCDECL_THIS_TAIL];
}

/**
 * What acquire() returns, by asyncdecl_acquire_in_task(), when $what —
 * "Method Class::method()" — is called on an object and does not use $this.
 *
 * @return array{string, string}
 */
function asyncdecl_on_object(string $what): array
{
    return [\Error::class, $what . ASYNCDECL_ON_OBJECT_TAIL];
}

/** The tail of the message for an enum a callback reaches that is not preloaded. */
const ASYNCDECL_ENUM_TAIL = ' was not loaded by opcache.preload: every request that loads such an enum has '
    . 'a copy of its own, cases included, and on a thread other than the one that created the pool a '
    . 'Shared\Pool callback would reach the copy of the request that created it, so the code was not run';

/** The tail of the message for a callback whose code is not in OPcache's shared memory. */
const ASYNCDECL_SHM_TAIL = " is not in OPcache's shared memory: on a thread other than the one that "
    . 'created the pool, a Shared\Pool callback runs only when its code is there, so the code was not run';

/**
 * What acquire() returns, by asyncdecl_acquire_in_task(), when the code of
 * $what — "Function name()", "Method Class::method()", or a closure — is not
 * in OPcache's shared memory.
 *
 * @return array{string, string}
 */
function asyncdecl_outside_shm(string|\Closure $what): array
{
    if ($what instanceof \Closure) {
        $r = new \ReflectionFunction($what);
        $what = 'The closure at ' . $r->getFileName() . ':' . $r->getStartLine();
    }
    return [\Error::class, $what . ASYNCDECL_SHM_TAIL];
}
