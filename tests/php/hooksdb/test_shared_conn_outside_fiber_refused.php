<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_conn_outside_fiber_refused', 'hooksdb');

// PHP does not only run inside request fibers. The worker loop collects cycles
// every hundred requests, and a destructor in a collected cycle is ordinary PHP
// that runs there, on the loop's own stack, while other requests are parked on
// connections the application shares. Such code can suspend into nothing, so it
// cannot wait for a connection — but it must still see a holder parked on one for
// its reply, or its command lands in the middle of that exchange.
//
// Three holders. Two are mid-exchange, each parked in a blocking pop: one through
// phpredis, one through a hand-written RESP exchange on a plain stream. The third
// used its phpredis connection and then parked on a sleep, so it holds that
// connection's client-level claim with nothing on the wire. This request drops a
// cycle whose destructor uses all three connections, and fills the worker's request
// count up to the next multiple of a hundred so the loop collects it while all
// three are still parked. Then it releases the holders itself, so the run takes as
// long as the fill does rather than the holders' twelve-second window.
$fetch = static function (string $path): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET {$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $response = (string) stream_get_contents($sock);
    fclose($sock);

    // The body alone: the holders' answers are compared exactly.
    $split = strpos($response, "\r\n\r\n");

    return $split === false ? $response : substr($response, $split + 4);
};

unset($sharedState['outside_fiber_redis_done'], $sharedState['outside_fiber_raw_done'],
      $sharedState['outside_fiber_redis_idle_done'], $sharedState['outside_fiber_probe'],
      $sharedState['outside_fiber_release']);

// A value the destructor's GET would return. If that command reaches the wire of a
// connection mid-exchange, its reply is the next thing there after the pop's, and a
// holder that reads it gets this string back in place of its own answer.
$seed = new Redis();
$seed->connect(getenv('DB_REDIS_HOST') ?: 'hooksdb-redis', 6379, 3.0);
$seed->set('hooksdb:outside:probe', 'outside-fiber-probe-value');
$seed->del('hooksdb:outside:empty', 'hooksdb:outside:raw');
$seed->close();

$hold = '/tests/hooksdb/fixture_outside_fiber_hold.php?mode=';
$redisHold = oxphp_async($fetch, $hold . 'redis');
$idleHold = oxphp_async($fetch, $hold . 'redis-idle');
$rawHold = oxphp_async($fetch, $hold . 'raw');

oxphp_usleep(500_000);

$t->assertTrue('the phpredis holder is parked on its connection',
    isset($sharedState['outside_fiber_redis']) && !isset($sharedState['outside_fiber_redis_done']));
$t->assertTrue('the idle phpredis holder is parked on a sleep',
    isset($sharedState['outside_fiber_redis_idle']) && !isset($sharedState['outside_fiber_redis_idle_done']));
$t->assertTrue('the raw-stream holder is parked on its connection',
    isset($sharedState['outside_fiber_raw']) && !isset($sharedState['outside_fiber_raw_done']));

// The cycle. Anonymous, so running this test twice on one worker does not
// redeclare a class. The closure keeps $sharedState by reference: that is where
// the connections are, and where the destructor leaves its account.
(function () use (&$sharedState, &$requestCount): void {
    $o = new class {
        public ?object $self = null;
        public ?\Closure $onDestruct = null;

        public function __destruct()
        {
            ($this->onDestruct)();
        }
    };
    $o->self = $o;
    $o->onDestruct = static function () use (&$sharedState, &$requestCount): void {
        $probe = [
            'request_count' => $requestCount,
            'redis_holder_done' => isset($sharedState['outside_fiber_redis_done']),
            'idle_holder_done' => isset($sharedState['outside_fiber_redis_idle_done']),
            'raw_holder_done' => isset($sharedState['outside_fiber_raw_done']),
        ];

        $raw = $sharedState['outside_fiber_raw'];
        $written = @fwrite($raw, "*2\r\n\$3\r\nGET\r\n\$21\r\nhooksdb:outside:probe\r\n");
        $probe['raw_write'] = $written;
        $probe['raw_timed_out'] = stream_get_meta_data($raw)['timed_out'];

        // Every phpredis outcome is recorded rather than asserted here, including
        // an exception: one left uncaught on the worker loop would surface in
        // whichever request is resumed next.
        foreach (['redis' => 'outside_fiber_redis', 'idle' => 'outside_fiber_redis_idle'] as $name => $key) {
            $started = microtime(true);
            try {
                $probe["{$name}_result"] = var_export($sharedState[$key]->get('hooksdb:outside:probe'), true);
            } catch (\Throwable $e) {
                $probe["{$name}_threw"] = get_class($e) . ': ' . $e->getMessage();
            }
            $probe["{$name}_seconds"] = round(microtime(true) - $started, 3);
        }

        $sharedState['outside_fiber_probe'] = $probe;
    };
})();
// The only references to the object are its own, so it is now a cycle only the
// collector can free.

// Fill up to the next multiple of a hundred. Each filler is one request on this
// worker; the one that lands on the multiple is followed by a collection on the
// worker loop, outside any request fiber.
$fillers = 100 - ($requestCount % 100);
$t->meta('fillers', $fillers);
$fillStarted = microtime(true);
$filled = oxphp_async(static function (int $n): int {
    $done = 0;
    for ($i = 0; $i < $n; $i++) {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            break;
        }
        stream_set_timeout($sock, 20);
        fwrite($sock, "GET /?action=gc_filler HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream_get_contents($sock);
        fclose($sock);
        $done++;
    }

    return $done;
}, $fillers);
$t->assertSame('every filler request was served', oxphp_async_await($filled), $fillers);
$t->meta('fill_seconds', round(microtime(true) - $fillStarted, 3));

oxphp_usleep(200_000);

// Release the holders: a value on each list ends the two pops with a reply of their
// own, and the flag ends the idle holder's sleep. Pushed on a connection of its own,
// after the destructor has had its turn.
$release = new Redis();
$release->connect(getenv('DB_REDIS_HOST') ?: 'hooksdb-redis', 6379, 3.0);
$release->rPush('hooksdb:outside:empty', 'released');
$release->rPush('hooksdb:outside:raw', 'released');
$release->close();
$sharedState['outside_fiber_release'] = true;

$redisBody = oxphp_async_await($redisHold);
$idleBody = oxphp_async_await($idleHold);
$rawBody = oxphp_async_await($rawHold);

$probe = $sharedState['outside_fiber_probe'] ?? null;
$t->assertTrue('the destructor ran', is_array($probe));
$probe = is_array($probe) ? $probe : [];
$t->meta('probe', $probe);

// Premises: the destructor ran from the loop's collection — on the hundredth
// request, not from an automatic collection inside some request — and while all
// three holders were still parked. Without these the assertions below could pass
// on a run that exercised nothing.
$t->assertSame('the destructor ran on a request count the loop collects on',
    ($probe['request_count'] ?? -1) % 100, 0);
$t->assertFalse('the phpredis holder was still parked when the destructor ran',
    $probe['redis_holder_done'] ?? true);
$t->assertFalse('the idle phpredis holder was still parked when the destructor ran',
    $probe['idle_holder_done'] ?? true);
$t->assertFalse('the raw-stream holder was still parked when the destructor ran',
    $probe['raw_holder_done'] ?? true);

// Mid-exchange, raw stream: the write was refused the way a timeout is, and
// nothing was sent.
$t->assertSame('the raw write outside a fiber sent nothing', $probe['raw_write'] ?? null, false);
$t->assertTrue('and reported a timeout', $probe['raw_timed_out'] ?? false);
$t->assertSame('the raw-stream holder read only its own reply', $rawBody,
    'raw-hold-done:' . json_encode("*2\r\n\$19\r\nhooksdb:outside:raw\r\n\$8\r\nreleased\r\n"));

// Mid-exchange, phpredis: its command was refused at the socket, so it failed at
// once instead of waiting on the wire behind the holder's pop and taking that
// reply as its own.
$t->assertSame('the phpredis call on a connection mid-exchange failed',
    $probe['redis_result'] ?? ($probe['redis_threw'] ?? ''), var_export(false, true));
$t->assertLessThan('and failed without waiting on the wire', $probe['redis_seconds'] ?? 99.0, 1.0);
$t->assertSame('the phpredis holder got the answer to its own pop', $redisBody,
    'redis-hold-done:' . var_export(['hooksdb:outside:empty', 'released'], true));

// Held but idle: the holder's claim lasts to the end of its request, yet nothing
// is on the wire, so a call from outside a fiber is safe and must go through.
$t->assertSame('a call on a connection held but idle went through',
    $probe['idle_result'] ?? ($probe['idle_threw'] ?? ''), var_export('outside-fiber-probe-value', true));
$t->assertSame('the idle holder finished normally', $idleBody, 'redis-idle-done');

$t->done();
