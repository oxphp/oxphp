<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool bound to a name with Registry::pool() runs its factory on each worker
// thread that acquires from it. This one comes from a file only this request
// includes, and calls a function of that file: on the other worker thread,
// which never included it, the acquire throws an Error naming the function and
// its call site, where it used to take the whole server down.
//
// The other thread is reached with a request to this server while this one
// holds its own worker reading the response; the profile runs two workers.
$test = new TestCase('pool_factory_from_file_peer_never_loaded', 'worker');

$factory = require __DIR__ . '/pool_lazy_factory.php';
$pool = \OxPHP\Shared\Registry::pool(
    'worker-lazy-factory-pool',
    static fn () => new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 2),
);
$handle = $pool->acquire();
$test->assertSame('the factory works on the thread that included its file', $handle->get()->v, 11);
$handle->release();

$body = @file_get_contents('http://127.0.0.1/tests/worker/pool_lazy_factory_peer.php');
$peer = is_string($body) ? json_decode($body, true) : null;
$test->assertTrue('the other worker answered', is_array($peer));
$test->assertSame('the other worker never included the file', $peer['helper_here'] ?? null, false);
$test->assertSame('its acquire throws an Error naming the function and its call site', $peer['result'] ?? null,
    'Error: Call to undefined function worker_lazy_pool_helper() at '
    . __DIR__ . '/pool_lazy_factory.php:' . WORKER_LAZY_POOL_CALL_LINE
    . ': it belongs to a script this PHP thread has not loaded; only built-in functions and classes'
    . ' and those loaded by opcache.preload exist on every PHP thread, so the code was not run');

// Unbound while this worker thread still runs: the factory lives in this
// thread's memory, and a pool still bound when the server stops is freed after
// the thread has exited, which crashes the process on the way out.
\OxPHP\Shared\Registry::remove('worker-lazy-factory-pool');

$test->done();
