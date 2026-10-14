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
