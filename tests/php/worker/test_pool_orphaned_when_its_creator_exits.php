<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/pool_orphan_lib.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A pool bound to a name keeps serving the workers that did not create it after
// the one that did has exited: the slots idle on them are still handed out. Its
// factory and $destroy went with the creator's memory, so no new slot is made
// and an evicted slot is let go without $destroy, both until Registry::pool() is
// called with the name again. That call gives the pool the caller's factory and
// $destroy.
//
// ?action=trigger creates the pool and has the other worker — reached with a
// request to this server while this one holds its own worker reading the
// response, as the profile runs two — make a slot with the creator's factory
// and keep it idle there. Then this worker exits.
//
// ?action=check waits until the creator has shut down, and runs the check on
// the worker that holds the pool: here, or through the other worker.

$marker = '/tmp/oxphp-worker-pool-orphan.txt';

if (!class_exists('OxphpPoolOrphanCreatorSentinel', false)) {
    final class OxphpPoolOrphanCreatorSentinel
    {
        public static ?self $kept = null;

        public function __destruct()
        {
            oxphp_pool_orphan_state()->set('creator_shut_down', true);
        }
    }
}

$t = new TestCase('pool_orphaned_when_its_creator_exits', 'worker');

if (($_GET['action'] ?? '') === 'trigger') {
    $name = 'worker-pool-orphan-' . bin2hex(random_bytes(4));
    file_put_contents($marker, $name);

    $pool = Registry::pool($name, static fn (): Pool => new Pool(
        factory: static fn (): \stdClass => (object) ['madeBy' => 'creator'],
        destroy: 'oxphp_pool_orphan_destroy',
        maxSize: 4,
    ));

    $body = @file_get_contents("http://127.0.0.1/tests/worker/pool_orphan_peer.php?action=mint&pool=$name");
    $peer = is_string($body) ? json_decode($body, true) : null;
    $t->assertTrue('the other worker answered', is_array($peer));
    $t->assertSame('it made a slot with the creator\'s factory', $peer['minted'] ?? $peer, 'creator');
    $t->assertSame('and keeps it idle there', $peer['idle'] ?? null, 1);

    OxphpPoolOrphanCreatorSentinel::$kept = new OxphpPoolOrphanCreatorSentinel();
    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$name = (string) @file_get_contents($marker);
$t->assertTrue('the trigger wrote the marker', str_starts_with($name, 'worker-pool-orphan-'));

// The creator's destructors run inside its shutdown, after the point from which
// the pool no longer uses its callables on other threads. The extra wait lets
// the rest of that shutdown, and the thread, finish.
$until = hrtime(true) + 5_000_000_000;
while (oxphp_pool_orphan_state()->get('creator_shut_down') !== true && hrtime(true) < $until) {
    usleep(20_000);
}
$t->assertTrue('the creator shut down', oxphp_pool_orphan_state()->get('creator_shut_down') === true);
usleep(300_000);

if (OxphpPoolOrphanHolder::$pool !== null) {
    $r = oxphp_pool_orphan_check($name);
} else {
    $body = @file_get_contents("http://127.0.0.1/tests/worker/pool_orphan_peer.php?action=check&pool=$name");
    $r = is_string($body) ? json_decode($body, true) : null;
}
$t->assertTrue('the worker holding the pool answered', is_array($r));
$r = is_array($r) ? $r : [];

$t->assertSame('it still holds the pool', $r['held'] ?? $r, true);
$t->assertSame('the slot idle on it is still handed out', $r['idle_slot'] ?? null, 'creator');
$t->assertSame(
    'a new slot is refused, naming what gives the pool a factory again',
    $r['new_slot'] ?? null,
    'OxPHP\Shared\ClosedException: Shared\Pool cannot make a new slot: its factory belonged to a '
    . 'PHP request that has ended. A pool bound to a name takes the factory and destroy callbacks '
    . 'of the next Shared\Registry::pool() or Shared\Registry::global() call for that name'
);
$t->assertSame('evict() lets the idle slot go', $r['evicted'] ?? null, 1);
$t->assertSame('without the creator\'s $destroy, counted as skipped', $r['skipped'] ?? null, 1);
$t->assertSame('the creator\'s $destroy never ran', $r['creator_destroy_ran'] ?? null, false);
$t->assertSame('Registry::pool() returns the same pool', $r['same_pool'] ?? null, true);
$t->assertSame('which now makes slots with the caller\'s factory', $r['slot_after_registry'] ?? null, 'peer');
$t->assertSame('and evicts them with the caller\'s $destroy', $r['evicted_after_registry'] ?? null, 1);
$t->assertSame('which ran', $r['peer_destroy_ran'] ?? null, true);

$t->done();
