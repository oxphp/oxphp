<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Counter;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Pool::evict() takes every idle slot of the calling thread off its deque and
// runs $destroy on each in turn. A fatal inside $destroy has to end the request
// as it would anywhere else, and the slots evict() had taken but not yet
// reached have to stay in the pool: otherwise they and the budget they hold are
// gone for the life of the process.
//
// A $destroy may also use the pool it is destroying a slot of. evict() must
// not be holding anything that use waits on.

$t = new TestCase('shared_pool_destroy_fatal', 'fibers');

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

// ── A fatal in $destroy ──────────────────────────────────────────────────────
// The fixture parks two slots and calls evict(); $destroy fatals on the older
// one, the first evict() reaches.

$resp = $get("case=pool_destroy&run=$run");
$t->assertMatch('the fatal ended the inner request with a 500', $resp, '#^HTTP/\S+ 500 #');
$t->assertNotContains('nothing past the fatal ran', $resp, 'NOT-REACHED');

// A worker retires after three requests in a row end in a bailout. A request
// that completes in between keeps this one in service.
$t->assertMatch('a request after it completes', $get('case=none'), '#^HTTP/\S+ 200 #');

$calls = Registry::counter("closure-fatal-pool-destroy-calls-$run", static fn (): Counter => new Counter());
$p = Registry::pool(
    "closure-fatal-pool-destroy-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 2)
);

// The slot the fatal was destroying counts as gone, and its budget with it;
// the one evict() never reached is idle again. This profile runs one worker,
// so this request is on the thread both slots belong to.
$stats = $p->stats();
$t->assertSame('the pool holds one slot', $stats->size(), 1);
$t->assertSame('that slot is idle', $stats->idle(), 1);

// Nothing ran after the fatal: $destroy was not called on the other slot.
$t->assertSame('$destroy ran once', $calls->get(), 1);

$kept = $p->tryAcquire();
$t->assertSame('the slot evict() did not reach is handed out', $kept?->get()->name ?? null, 'newer');

$fresh = $p->tryAcquire();
$t->assertNotNull('the budget of the destroyed slot mints a new one', $fresh);
$t->assertNull('and that one is new', $fresh?->get()->name ?? null);

// Both go back to the deque the fatal was emptying.
$kept?->release();
$fresh?->release();
$t->assertSame('evict() destroys both', $p->evict(), 2);
$t->assertSame('$destroy ran on both', $calls->get(), 3);

// ── $destroy uses the pool ───────────────────────────────────────────────────
// Last: an evict() that waits on itself never returns, and this profile has one
// worker to lose. Room for a third slot, so the acquire in $destroy mints one
// and its release parks it in the deque evict() is draining.

// Not made in an arrow function: that would bind $reentrant by value, and
// $destroy would see the null it held then. A throw in $destroy is swallowed,
// so that mistake would pass for the pool working.
$reentrant = null;
$reentrant = new Pool(
    factory: static fn (): \stdClass => new \stdClass(),
    destroy: static function (\stdClass $r) use (&$reentrant): void {
        $reentrant?->tryAcquire()?->release();
    },
    maxSize: 3,
);
$a = $reentrant->tryAcquire();
$b = $reentrant->tryAcquire();
$a->release();
$b->release();
unset($a, $b);

$t->assertSame('a $destroy that acquires and releases lets evict() return', $reentrant->evict(), 2);
$t->assertSame('and leaves the slot it released idle', $reentrant->stats()->idle(), 1);

// The pool holds $destroy, and $destroy holds this variable: emptying it lets
// the pool go, which its own callback would otherwise keep alive.
$reentrant = null;

$t->done();
