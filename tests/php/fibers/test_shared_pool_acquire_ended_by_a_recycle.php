<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A request waiting in Shared\Pool::acquire() that its worker ends where it
// waits — a worker that exits ends the requests still parked on it — is ended
// by a bailout raised at the park, and the pool stops counting it as waiting.
// The bailout has to be caught short of the acquire's own frames and raised
// again past them: one that jumped straight across them would leave the pool
// counting a waiter that is gone, until its thread exits — through the
// waiter's own shutdown, which is where this looks.
//
// ?action=trigger holds the pool's one slot, sends the waiter, sees it waiting,
// and has its worker exit. The slot and the waiter's connection are kept past
// this request, so it is the exit that ends the waiter: not the slot coming
// back, nor its client going away.
//
// ?action=check runs on the worker that replaced it, and reads what the
// waiter's shutdown function wrote and what the pool counts now.

$marker = '/tmp/oxphp-fibers-pool-acquire-recycled.txt';

// Declared conditionally: this file runs once per request on a worker that
// keeps its classes.
if (!class_exists('OxphpPoolAcquireRecycleProbe', false)) {
    final class OxphpPoolAcquireRecycleProbe
    {
        /** The slot the trigger holds past its own end. */
        public static mixed $slot = null;

        /** @var resource|null The waiter's connection, kept open likewise. */
        public static $heldOpen = null;
    }
}

$t = new TestCase('shared_pool_acquire_ended_by_a_recycle', 'fibers');

if (($_GET['action'] ?? '') === 'trigger') {
    $name = 'pool-acquire-recycled-' . bin2hex(random_bytes(4));
    file_put_contents($marker, "$name\n");

    $p = Registry::pool(
        $name,
        static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), maxSize: 1)
    );
    OxphpPoolAcquireRecycleProbe::$slot = $p->tryAcquire();
    $t->assertNotNull('this request holds the one slot', OxphpPoolAcquireRecycleProbe::$slot);

    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('the waiter was sent', $sock !== false);
    if ($sock !== false) {
        fwrite($sock, "GET /tests/fibers/fixture_shared_pool_acquire_recycled.php?pool=$name HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
        OxphpPoolAcquireRecycleProbe::$heldOpen = $sock;
    }

    $seen = 0;
    $until = hrtime(true) + 2_000_000_000;
    while ($seen === 0 && hrtime(true) < $until) {
        oxphp_usleep(10_000);
        $seen = $p->stats()->waiting();
    }
    $t->assertSame('the waiter waits, and lets this request run meanwhile', $seen, 1);

    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$lines = file($marker, FILE_IGNORE_NEW_LINES) ?: [];
$name = $lines[0] ?? '';
$t->assertTrue('the trigger wrote the marker', str_starts_with($name, 'pool-acquire-recycled-'));

$waiter = json_decode($lines[1] ?? 'null', true);
$t->assertSame(
    'the waiter was ended where it waited, by the exit',
    $waiter['message'] ?? null,
    'Request cancelled (shutdown)'
);
$t->assertSame('its shutdown function no longer saw it waiting', $waiter['waiting'] ?? null, 0);

$t->assertTrue('the pool outlived the worker', in_array($name, Registry::keys(), true));
if (in_array($name, Registry::keys(), true)) {
    $p = Registry::pool($name, static fn (): Pool => throw new \LogicException('the pool is gone'));
    $t->assertSame('nor does the pool, on the worker that replaced it', $p->stats()->waiting(), 0);
}

$t->done();
