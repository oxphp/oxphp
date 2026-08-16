<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('nested_composition', 'async');

// A task that itself dispatches and awaits a nested task (async composition).
// The outer task suspends at its await, freeing the worker to run the nested
// task; the outer fiber resumes once the nested promise resolves.
$p = oxphp_async(function (): int {
    $inner = oxphp_async(fn () => 21);
    return oxphp_async_await($inner) * 2;
});

$r = oxphp_async_await($p);
$t->assertSame('nested oxphp_async + await composes (21 * 2)', $r, 42);

$t->done();
