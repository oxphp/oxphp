<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A pool bound to a name with Registry::pool() runs its factory on each worker
// thread that acquires from it. A method of an enum opcache.preload did not load
// keeps its `static` variables in the memory of the request that linked the
// enum, and a name that reaches it on the other worker is refused there: a string
// that thread appended to one of them, in its own memory, would be freed by this
// one, taking the whole server down. With no `static` it runs there.
//
// The other thread is reached with a request to this server while this one
// holds its own worker reading the response; the profile runs two workers.
$test = new TestCase('pool_named_callback_to_a_linked_method_on_peer_worker', 'worker');

enum WorkerNamedLinkedSuit
{
    case Hearts;

    public static function appended(): object
    {
        static $log = '';
        $log .= str_repeat('x', 2);
        return (object) ['v' => $log];
    }

    public static function plain(): object
    {
        return (object) ['v' => 'plain'];
    }
}

function worker_named_linked_peer(string $pool): mixed
{
    $body = @file_get_contents("http://127.0.0.1/tests/worker/pool_captures_peer.php?step=acquire&pool=$pool");
    $peer = is_string($body) ? json_decode($body, true) : null;
    return is_array($peer) ? ($peer['result'] ?? null) : null;
}

WorkerNamedLinkedSuit::appended();
$appended = \OxPHP\Shared\Registry::pool(
    'worker-named-linked-appended',
    static fn () => new \OxPHP\Shared\Pool(factory: [WorkerNamedLinkedSuit::class, 'appended'], destroy: null, maxSize: 2),
);
$plain = \OxPHP\Shared\Registry::pool(
    'worker-named-linked-plain',
    static fn () => new \OxPHP\Shared\Pool(factory: [WorkerNamedLinkedSuit::class, 'plain'], destroy: null, maxSize: 2),
);

$test->assertSame(
    'a method with a static: refused on the other worker',
    worker_named_linked_peer('worker-named-linked-appended'),
    [\Error::class . ': Method WorkerNamedLinkedSuit::appended() keeps its static variables in the memory of the '
        . 'request that created the pool: on a thread other than that one, a Shared\Pool callback cannot use them, '
        . 'so the code was not run'],
);
$test->assertSame(
    'a method with no static: runs on the other worker',
    worker_named_linked_peer('worker-named-linked-plain'),
    ['plain', 'plain'],
);

$handle = $appended->acquire();
$test->assertSame("on this thread the factory appends to the method's", $handle->get()->v, 'xxxx');
$handle->release();
$test->assertSame("the method appends to the factory's", WorkerNamedLinkedSuit::appended()->v, 'xxxxxx');

// Unbound while this worker thread still runs, as the pools' factories live in
// this thread's memory.
\OxPHP\Shared\Registry::remove('worker-named-linked-appended');
\OxPHP\Shared\Registry::remove('worker-named-linked-plain');
unset($handle, $appended, $plain);

$test->done();
