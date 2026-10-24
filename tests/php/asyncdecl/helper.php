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
