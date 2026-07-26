<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_shapes', 'hooks');

// The argument shapes the multiplexing tests never reach: a write array, an
// exception array, an element that is not a live stream, and an unbounded wait.
// Each runs once in the request context, where no fiber exists and the hook
// delegates on its first line, and once inside an async task, where it takes the
// hooked path. The two columns must agree.
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
    $peer = stream_socket_accept($server, 3.0);

    // 1. A write array on its own. A freshly connected socket has room in its
    //    send buffer, so this is answered at once from POLLOUT.
    $read = null;
    $write = [$client];
    $except = null;
    $t0 = microtime(true);
    $writable = stream_select($read, $write, $except, 2);
    $writableElapsed = microtime(true) - $t0;
    $writableKept = is_array($write) && count($write) === 1;

    // 2. An unbounded wait on a socket that already has a byte waiting. The
    //    deadline is the one the wait primitive represents as "never", so a hook
    //    that mishandles it hangs the task instead of returning.
    fwrite($peer, 'x');
    $read2 = [$client];
    $t1 = microtime(true);
    $unbounded = stream_select($read2, $write2, $except2, null);
    $unboundedElapsed = microtime(true) - $t1;
    fread($client, 1);

    // 3. An exception array on a descriptor that has been shut down in both
    //    directions. poll() reports the hangup whether or not anyone asked for
    //    it, but PHP's select() maps a hangup onto the read set alone and never
    //    onto the exception set — so this call has nothing to report and must
    //    spend its whole timeout, exactly as it does without the hook.
    stream_socket_shutdown($client, STREAM_SHUT_RDWR);
    $read3 = null;
    $write3 = null;
    $except3 = [$client];
    $t2 = microtime(true);
    $hangup = stream_select($read3, $write3, $except3, 1);
    $hangupElapsed = microtime(true) - $t2;

    fclose($client);
    fclose($peer);

    // 4. A stream closed and left in the array — the shape a long-lived select
    //    loop grows by accident. Native raises an exception for it while
    //    selecting on the rest, and an exception is nothing a wait can stand in
    //    for: the hook has to hand the call over rather than park a fiber with
    //    one already pending. The live socket beside it is made readable so the
    //    two columns are compared on the error, not on a timeout.
    $live = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    $peer2 = stream_socket_accept($server, 3.0);
    fwrite($peer2, 'z');
    $dead = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    $peer3 = stream_socket_accept($server, 3.0);
    fclose($dead);

    $read4 = [$dead, $live];
    $closedError = null;
    $t3 = microtime(true);
    try {
        stream_select($read4, $write4, $except4, 2);
    } catch (\Throwable $ex) {
        $closedError = $ex::class . ': ' . $ex->getMessage();
    }
    $closedElapsed = microtime(true) - $t3;

    fclose($live);
    fclose($peer2);
    fclose($peer3);
    fclose($server);

    return [
        'error' => null,
        'writable' => $writable,
        'writableElapsed' => $writableElapsed,
        'writableKept' => $writableKept,
        'unbounded' => $unbounded,
        'unboundedElapsed' => $unboundedElapsed,
        'hangup' => $hangup,
        'hangupElapsed' => $hangupElapsed,
        'closedError' => $closedError,
        'closedElapsed' => $closedElapsed,
    ];
};

$native = $probe();
$hooked = oxphp_async_await(oxphp_async($probe), 15.0);

foreach (['native' => $native, 'hooked' => $hooked] as $label => $r) {
    $t->assertNull("{$label}: the probe set up its sockets", $r['error']);

    $t->assertSame("{$label}: the socket reported writable", $r['writable'], 1);
    $t->assertTrue("{$label}: the write array kept the socket", $r['writableKept']);
    $t->assertLessThan("{$label}: the write case answered at once", $r['writableElapsed'], 0.5);

    $t->assertSame("{$label}: the unbounded wait found the readable socket", $r['unbounded'], 1);
    $t->assertLessThan("{$label}: the unbounded wait returned at once", $r['unboundedElapsed'], 0.5);

    $t->assertSame("{$label}: a hangup is not an exception-set event", $r['hangup'], 0);
    $t->assertGreaterThan("{$label}: the hangup case waited out its 1s timeout", $r['hangupElapsed'], 0.9);
    $t->assertLessThan("{$label}: the hangup case did not wait twice", $r['hangupElapsed'], 1.8);

    $t->assertContains(
        "{$label}: a closed stream in the array raised native's own error",
        (string) $r['closedError'],
        'TypeError: stream_select(): supplied resource is not a valid stream resource'
    );
    $t->assertLessThan("{$label}: the closed-stream case did not wait", $r['closedElapsed'], 0.5);
}

$t->assertSame('hooked write result matches the native one', $hooked['writable'], $native['writable']);
$t->assertSame('hooked hangup result matches the native one', $hooked['hangup'], $native['hangup']);
$t->assertSame('hooked closed-stream error matches the native one', $hooked['closedError'], $native['closedError']);

$t->done();
