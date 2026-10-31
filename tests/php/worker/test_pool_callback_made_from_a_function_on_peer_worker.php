<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool bound to a name with Registry::pool() runs its factory on each worker
// thread that acquires from it. A factory made from a function, as f(...)
// makes one, changed that function's `static` variables on the other worker
// thread as this request has them, in its memory, and appending to a string
// there took the whole server down. There it runs as the function itself, with
// that thread's own `static`s.
//
// The other thread is reached with a request to this server while this one
// holds its own worker reading the response; the profile runs two workers.
$test = new TestCase('pool_callback_made_from_a_function_on_peer_worker', 'worker');

function worker_made_from_appended(): object
{
    static $log = '';
    $log .= str_repeat('x', 2);
    return (object) ['v' => $log];
}

worker_made_from_appended();
$pool = \OxPHP\Shared\Registry::pool(
    'worker-made-from-pool',
    static fn () => new \OxPHP\Shared\Pool(factory: worker_made_from_appended(...), destroy: null, maxSize: 2),
);

$body = @file_get_contents('http://127.0.0.1/tests/worker/pool_captures_peer.php?step=acquire&pool=worker-made-from-pool');
$peer = is_string($body) ? json_decode($body, true) : null;
$test->assertTrue('the other worker answered', is_array($peer));
$test->assertSame('the other worker appends to its own', $peer['result'] ?? null, ['xx', 'xxxx']);

$handle = $pool->acquire();
$test->assertSame("on this thread the factory appends to the function's", $handle->get()->v, 'xxxx');
$handle->release();

// Unbound while this worker thread still runs, as the pool's factory lives in
// this thread's memory.
\OxPHP\Shared\Registry::remove('worker-made-from-pool');
unset($handle, $pool);

$test->done();
