<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_timeout_semantics', 'hooks');

// A listening socket that is never accepted from: the client's connect completes
// through the kernel backlog, but no byte ever arrives, so the read runs into
// the stream's own timeout. Everything observable about that timeout — the empty
// read, the timed_out metadata flag, the wall clock — must be identical whether
// the read went through the hook or straight to the native handler.
$probe = static function (): array {
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    if ($server === false) {
        return ['error' => "server: {$errstr} ({$errno})"];
    }
    $addr = stream_socket_get_name($server, false);

    $client = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    if ($client === false) {
        fclose($server);

        return ['error' => "client: {$errstr} ({$errno})"];
    }
    stream_set_timeout($client, 1);

    $t0 = microtime(true);
    $data = fread($client, 16);
    $elapsed = microtime(true) - $t0;
    $meta = stream_get_meta_data($client);

    fclose($client);
    fclose($server);

    return ['error' => null, 'data' => $data, 'elapsed' => $elapsed, 'timed_out' => $meta['timed_out']];
};

// Traditional mode: the request runs outside any fiber, so the hook delegates
// straight to the native handler.
$native = $probe();
// An async task runs inside a fiber, so the same code takes the hooked path.
$hooked = oxphp_async_await(oxphp_async($probe), 5.0);

foreach (['native' => $native, 'hooked' => $hooked] as $label => $r) {
    $t->assertNull("{$label}: the probe set up its sockets", $r['error']);
    // A read that hits the socket timeout fails rather than returning bytes.
    $t->assertSame("{$label}: the timed-out read reported failure", $r['data'], false);
    $t->assertTrue("{$label}: stream metadata reports timed_out", $r['timed_out'] === true);
    $t->assertGreaterThan("{$label}: the read waited out its 1s timeout", $r['elapsed'], 0.9);
    // The upper bound exists to catch the hook waiting once itself and then
    // letting the native path wait the same timeout again, which lands at 2.0s.
    // 1.8 keeps 0.8s of slack for a loaded machine and still fails that bug.
    $t->assertLessThan("{$label}: the read did not wait its timeout twice", $r['elapsed'], 1.8);
}

// The point of the pair: the hook must not be observable in the result.
$t->assertSame('hooked read result matches the native one', $hooked['data'], $native['data']);
$t->assertSame('hooked timed_out flag matches the native one', $hooked['timed_out'], $native['timed_out']);

$t->done();
