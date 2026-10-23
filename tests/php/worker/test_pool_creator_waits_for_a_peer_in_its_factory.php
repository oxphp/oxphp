<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A worker that exits while another worker is inside the factory of a pool the
// exiting one created waits for that call to return before it shuts its PHP
// request down: the factory is a callable of that request, and its memory goes
// with the shutdown.
//
// ?action=trigger creates the pool and sends a request to this server, which the
// other worker serves — the profile runs two — and which makes a slot there. The
// factory notes that it has started, sleeps, and notes on its way out whether
// the creator had shut down meanwhile. Once the factory has started, this worker
// exits, and an object it keeps notes, from its destructor, whether the factory
// had returned by then.
//
// ?action=check waits for both notes. Each is read at the moment it is about,
// not afterwards.

$marker = '/tmp/oxphp-worker-pool-wait.txt';

if (!class_exists('OxphpPoolWaitCreatorSentinel', false)) {
    final class OxphpPoolWaitCreatorSentinel
    {
        public static ?self $kept = null;

        /** @var resource|null The request to the other worker, kept open. */
        public static $peer = null;

        public function __destruct()
        {
            $s = Registry::map('worker-pool-wait-state', static fn (): Map => new Map());
            $s->set('factory_had_returned_at_creator_shutdown', $s->get('factory_returned') === true);
            $s->set('creator_shut_down', true);
        }
    }
}

$t = new TestCase('pool_creator_waits_for_a_peer_in_its_factory', 'worker');

$state = Registry::map('worker-pool-wait-state', static fn (): Map => new Map());

if (($_GET['action'] ?? '') === 'trigger') {
    $name = 'worker-pool-wait-' . bin2hex(random_bytes(4));
    file_put_contents($marker, $name);

    $pool = Registry::pool($name, static fn (): Pool => new Pool(
        factory: static function (): \stdClass {
            $s = Registry::map('worker-pool-wait-state', static fn (): Map => new Map());
            $s->set('factory_started', true);
            usleep(600_000);
            $s->set('creator_had_shut_down_when_factory_returned', $s->get('creator_shut_down') === true);
            $s->set('factory_returned', true);
            return new \stdClass();
        },
        maxSize: 2,
    ));

    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('the request to the other worker was sent', $sock !== false);
    if ($sock !== false) {
        fwrite($sock, "GET /tests/worker/pool_wait_peer.php?pool=$name HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
        OxphpPoolWaitCreatorSentinel::$peer = $sock;
    }

    $until = hrtime(true) + 3_000_000_000;
    while ($state->get('factory_started') !== true && hrtime(true) < $until) {
        usleep(10_000);
    }
    $t->assertTrue('the other worker is inside the factory', $state->get('factory_started') === true);
    $t->assertTrue('and has not returned from it yet', $state->get('factory_returned') !== true);

    OxphpPoolWaitCreatorSentinel::$kept = new OxphpPoolWaitCreatorSentinel();
    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$name = (string) @file_get_contents($marker);
$t->assertTrue('the trigger wrote the marker', str_starts_with($name, 'worker-pool-wait-'));

$until = hrtime(true) + 6_000_000_000;
while (($state->get('creator_shut_down') !== true || $state->get('factory_returned') !== true)
    && hrtime(true) < $until) {
    usleep(20_000);
}
$t->assertTrue('the factory returned', $state->get('factory_returned') === true);
$t->assertTrue('the creator shut down', $state->get('creator_shut_down') === true);
$t->assertSame(
    'the creator had not shut down when the factory returned',
    $state->get('creator_had_shut_down_when_factory_returned'),
    false
);
$t->assertSame(
    'the factory had returned when the creator shut down',
    $state->get('factory_had_returned_at_creator_shutdown'),
    true
);

Registry::remove($name);
$t->done();
