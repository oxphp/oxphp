<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool bound to a name with Registry::pool() runs its factory on each worker
// thread that acquires from it. This factory comes from a file only this request
// includes, and reaches a method calling a function of that file only through
// a callable read from a global constant. On the other worker thread, which
// never included the file, the call throws an Error naming the function and its
// call site; call_user_func(), which is compiled to an opcode, throws a TypeError
// on top of it.
//
// The other thread is reached with a request to this server while this one
// holds its own worker reading the response; the profile runs two workers.
$test = new TestCase('pool_factory_reaches_scope_through_a_run_time_callable_on_a_peer', 'worker');

\defined('WORKER_RUN_TIME_CALLABLE') || \define('WORKER_RUN_TIME_CALLABLE', 'static::make');

$factory = require __DIR__ . '/pool_run_time_callable_factory.php';
$pool = \OxPHP\Shared\Registry::pool(
    'worker-run-time-callable-pool',
    static fn () => new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 2),
);
$handle = $pool->acquire();
$test->assertSame('the factory works on the thread that included its file', $handle->get()->v, 11);
$handle->release();

$body = @file_get_contents('http://127.0.0.1/tests/worker/pool_run_time_callable_peer.php');
$peer = is_string($body) ? json_decode($body, true) : null;
$test->assertTrue('the other worker answered', is_array($peer));
$test->assertSame('the other worker never included the file', $peer['helper_here'] ?? null, false);
$test->assertSame('its acquire throws what call_user_func() throws for a callable it cannot use', $peer['result'] ?? null,
    'TypeError: call_user_func(): Argument #1 ($callback) must be a valid callback, (null)');
$test->assertSame('on top of an Error naming the function and its call site', $peer['previous'] ?? null,
    'Error: Call to undefined function worker_run_time_callable_helper() at '
    . __DIR__ . '/pool_run_time_callable_factory.php:' . WORKER_RUN_TIME_CALLABLE_CALL_LINE
    . ': it belongs to a script this PHP thread has not loaded; only built-in functions and classes'
    . ' and those loaded by opcache.preload exist on every PHP thread, so the code was not run');

// Unbound while this worker thread still runs: the factory lives in this
// thread's memory, and a pool still bound when the server stops is freed after
// the thread has exited, which crashes the process on the way out.
\OxPHP\Shared\Registry::remove('worker-run-time-callable-pool');

$test->done();
