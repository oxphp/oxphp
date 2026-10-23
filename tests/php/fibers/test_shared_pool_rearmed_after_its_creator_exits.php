<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A pool bound to a name outlives the worker that created it, but its factory
// does not: the factory is a callable of that worker's PHP request, and the
// worker's memory goes with it. The worker that replaces it calls
// Registry::pool() with the name again, as its bootstrap would, and that call
// gives the pool its own factory: the pool and its name stay the same, and the
// next slot is made by the new factory.
//
// ?action=trigger creates the pool, makes a slot and gives it back, writes the
// pool's name and id to a marker, and has its worker exit.
//
// ?action=check runs on the worker that replaced it. The slot the trigger gave
// back went with its worker, so the acquire here has to make a new one.

$marker = '/tmp/oxphp-fibers-pool-rearmed.json';

$t = new TestCase('shared_pool_rearmed_after_its_creator_exits', 'fibers');

if (($_GET['action'] ?? '') === 'trigger') {
    $name = 'pool-rearmed-' . bin2hex(random_bytes(4));
    $p = Registry::pool($name, static fn (): Pool => new Pool(
        factory: static fn (): \stdClass => (object) ['madeBy' => 'creator'],
        maxSize: 2,
    ));
    $h = $p->acquire();
    $t->assertSame('the creator makes slots with its own factory', $h->get()->madeBy, 'creator');
    $h->release();
    file_put_contents($marker, json_encode(['name' => $name, 'id' => $p->id()]));

    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$m = json_decode((string) @file_get_contents($marker), true);
$t->assertTrue('the trigger wrote the marker', is_array($m));
$name = is_array($m) ? (string) $m['name'] : '';

$reentered = null;
$p = Registry::pool($name, static function () use ($name, &$reentered): Pool {
    // Asked for its own name, the factory gets the pool as it is, without
    // being run again.
    $reentered = Registry::pool($name, static fn (): Pool => throw new \LogicException('run again'));
    return new Pool(
        factory: static fn (): \stdClass => (object) ['madeBy' => 'replacement'],
        maxSize: 2,
    );
});
$t->assertSame('the name still holds the pool the exited worker created', $p->id(), $m['id'] ?? null);
$t->assertSame('the factory that gave it new callbacks got that pool for its name', $reentered?->id(), $m['id'] ?? null);

$h = $p->acquire();
$t->assertSame(
    'the pool makes slots with the factory of the worker that replaced its creator',
    $h->get()->madeBy,
    'replacement'
);
$h->release();
$t->assertSame('the slot is the only one the pool has', $p->stats()->size(), 1);

// The pool's last reference goes in this request, which made the factory the
// pool now holds: the factory is freed here, once.
Registry::remove($name);
unset($h, $p, $reentered);
$t->assertTrue('the pool went away with its adopted factory', true);

$t->done();
