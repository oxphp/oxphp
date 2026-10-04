<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Counter;
use OxPHP\Shared\Mutex;
use OxPHP\Shared\Once;
use OxPHP\Shared\Once\FailureMode;
use OxPHP\Shared\Once\Status;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Most Shared\* methods that run a callable for their caller hold something
// while it runs — the lock of a Mutex, the init lock of a Once, the creation
// slot of a Registry key, a slot of a Pool or the budget for one — and give it
// back when the callable returns or throws. A fatal inside the callable has to
// end the request exactly as it would anywhere else, and the method has to give
// back what it held all the same: otherwise it stays held for the life of the
// process, and every later caller waits on it or is refused.
//
// Each inner request below fatals inside one such callable. This request then
// asks for the same primitive again, from the worker thread the fatal happened
// on — this profile runs one, so this request is on it — and, where another
// thread would see something different, from an async task as well.

$t = new TestCase('shared_closure_fatal_releases', 'fibers');

$run = bin2hex(random_bytes(4));

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shared_closure_fatal.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked (this profile hooks streams): the read parks this request's fiber,
    // which leaves the worker free to serve the request just sent.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

$inner = static function (string $case, int $status = 500) use ($t, $run, $get): void {
    $resp = $get("case=$case&run=$run");

    // The fatal is still a fatal: it ends the request, and nothing after the
    // call it was raised in runs.
    $t->assertMatch("$case: the fatal ended the inner request with a $status", $resp, "#^HTTP/\\S+ $status #");
    $t->assertNotContains("$case: nothing past the fatal ran", $resp, 'NOT-REACHED');

    // A worker retires after three requests in a row end in a bailout, and this
    // test raises eleven on the one worker it then reads back from. A request
    // that completes in between keeps that worker in service.
    $t->assertMatch("$case: a request after it completes", $get('case=none'), '#^HTTP/\S+ 200 #');
};

/** What a call returned, or the exception it threw instead. */
$probe = static function (callable $fn): mixed {
    try {
        return $fn();
    } catch (\Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};

// ── Mutex ────────────────────────────────────────────────────────────────────
// The closure wrote to the state before the fatal. The write is not kept: the
// closure never finished, and only a closure that returns or throws hands its
// state back. with_lock_parked is the time limit running out while the closure
// is parked in a native sleep: the request is ended from there with a 504, and
// that too is a fatal inside the closure rather than a throw out of it.
// with_lock_destructor is a fatal after the closure has returned, from the
// destructor of the value it returned, which the Mutex could not store.

foreach (['with_lock' => 500, 'with_lock_parked' => 504, 'with_lock_destructor' => 500, 'try_with_lock' => 500] as $case => $status) {
    $inner($case, $status);
    $key = 'closure-fatal-' . str_replace('_', '-', $case) . "-$run";
    $m = Registry::mutex($key, static fn (): Mutex => new Mutex('not-created-by-the-fixture'));

    $t->assertSame(
        "$case: the worker thread the fatal was on takes the lock again",
        $probe(static fn (): mixed => $m->tryWithLock(static fn (mixed &$v): mixed => $v)),
        'clean'
    );

    $t->assertSame(
        "$case: another thread takes the lock",
        oxphp_async_await(oxphp_async(static function () use ($m): mixed {
            try {
                return $m->tryWithLock(static fn (mixed &$v): mixed => $v);
            } catch (\Throwable $e) {
                return get_class($e) . ': ' . $e->getMessage();
            }
        })),
        'clean'
    );
}

// ── Once ─────────────────────────────────────────────────────────────────────
// A fatal is not the factory failing — it is the request being ended — so even
// a Poison cell goes back to Uninitialized and the next caller runs its own
// factory.

$inner('once');
$o = Registry::once("closure-fatal-once-$run", static fn (): Once => new Once(FailureMode::Poison));
$status = $o->status();
$t->assertSame('once: the cell is Uninitialized again', $status->name, Status::Uninitialized->name);

// Only past that: on a cell left Pending, getOrInit waits for an init lock
// nothing is going to release, and this profile has one worker to lose.
if ($status !== Status::Pending) {
    $t->assertSame(
        'once: a later getOrInit runs its factory',
        $probe(static fn (): mixed => $o->getOrInit(static fn (): string => 'ready')),
        'ready'
    );
}

// The fatal comes from resolving the factory rather than from running it: the
// autoloader for the class it names. The init lock is already taken by then.

$inner('once_autoload');
spl_autoload_unregister('oxphp_shared_lazy_fatal_autoload');
$o = Registry::once("closure-fatal-once-autoload-$run", static fn (): Once => new Once(FailureMode::Poison));
$status = $o->status();
$t->assertSame('once_autoload: the cell is Uninitialized again', $status->name, Status::Uninitialized->name);
if ($status !== Status::Pending) {
    $t->assertSame(
        'once_autoload: a later getOrInit runs its factory',
        $probe(static fn (): mixed => $o->getOrInit(static fn (): string => 'ready')),
        'ready'
    );
}

// ── Registry ─────────────────────────────────────────────────────────────────
// The key the fatal was creating is free again: the next caller creates it.
// registry_destructor is a fatal after the factory has returned, from the
// destructor of the value it returned, which the Registry could not store.

foreach (['registry', 'registry_destructor'] as $case) {
    $inner($case);
    $key = 'closure-fatal-' . str_replace('_', '-', $case) . "-$run";
    $t->assertSame(
        "$case: the key the fatal was creating can be created",
        $probe(static fn (): mixed => get_class(Registry::counter($key, static fn (): Counter => new Counter()))),
        Counter::class
    );
}

// ── Pool ─────────────────────────────────────────────────────────────────────
// One slot per pool: whatever the fatal kept would be all of it.

$inner('pool_factory');
$p = Registry::pool(
    "closure-fatal-pool-factory-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 1)
);
$h = $p->tryAcquire();
$t->assertNotNull('pool_factory: the slot the fatal was creating can be created', $h);
$h?->release();

$inner('pool_with');
$p = Registry::pool(
    "closure-fatal-pool-with-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 1)
);
$h = $p->tryAcquire();
$t->assertNotNull('pool_with: the slot the body had is back in the pool', $h);
if ($h !== null) {
    // And nothing else still holds the resource the body was given: once the
    // pool lets it go, it is freed.
    $w = \WeakReference::create($h->get());
    $h->release();
    unset($h);
    $p->evict();
    $t->assertNull('pool_with: the resource is freed once the pool lets it go', $w->get());
}

// ── Map ──────────────────────────────────────────────────────────────────────
// Nothing to read back; the inner request's own assertions are the test.

$inner('for_each');

// The worker is still the one serving this request, so reaching the end at all
// says none of the fatals took it down.
$t->assertTrue('the worker still serves this request', oxphp_is_worker());

$t->done();
