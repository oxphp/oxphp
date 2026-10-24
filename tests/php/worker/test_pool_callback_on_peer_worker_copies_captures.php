<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool bound to a name with Registry::pool() runs its factory on each worker
// thread that acquires from it. On the other worker thread the factory ran
// with the variables of this request's closure: what it captured went into
// that thread's buffer of possible garbage roots, and its `static`, set there
// for the first time, into a table this request frees, which took the whole
// server down. There it runs with copies of them made on that thread, each
// call starting from their values when the pool was created.
//
// The other thread is reached with a request to this server while this one
// holds its own worker reading the response; the profile runs two workers.
$test = new TestCase('pool_callback_on_peer_worker_copies_captures', 'worker');

$config = ['prefix' => str_repeat('p', 2)];
$pool = \OxPHP\Shared\Registry::pool(
    'worker-captures-pool',
    static fn () => new \OxPHP\Shared\Pool(
        factory: static function () use ($config): object {
            static $made = 0;
            return (object) ['v' => $config['prefix'] . ++$made];
        },
        destroy: null,
        maxSize: 2,
    ),
);

$body = @file_get_contents('http://127.0.0.1/tests/worker/pool_captures_peer.php?step=acquire');
$peer = is_string($body) ? json_decode($body, true) : null;
$test->assertTrue('the other worker answered', is_array($peer));
$test->assertSame('each of its calls starts from the values of the pool\'s creation', $peer['result'] ?? null, ['pp1', 'pp1']);

$handle = $pool->acquire();
$test->assertSame('on this thread the factory counts on from its own calls only', $handle->get()->v, 'pp1');
$handle->release();

// Unbound while this worker thread still runs: the factory lives in this
// thread's memory, and a pool still bound when the server stops is freed after
// the thread has exited, which crashes the process on the way out.
\OxPHP\Shared\Registry::remove('worker-captures-pool');
unset($handle, $pool);

$body = @file_get_contents('http://127.0.0.1/tests/worker/pool_captures_peer.php?step=collect');
$collect = is_string($body) ? json_decode($body, true) : null;
$test->assertTrue('the other worker collected garbage after the pool went away', is_int($collect['collected'] ?? null));

$test->done();
