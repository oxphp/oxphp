<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_semantics', 'hooks');

// Everything observable about stream_select() must be identical whether the call
// went through the hook or straight to the native handler: the return count, the
// surviving contents of all three arrays, and — for a wait that finds nothing —
// the wall clock. In traditional mode the request runs outside any fiber, so the
// hook delegates; the same closure inside an async task takes the hooked path.
$probe = static function (): array {
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    if ($server === false) {
        return ['error' => "server: {$errstr} ({$errno})"];
    }
    $addr = stream_socket_get_name($server, false);

    $ready = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    if ($ready === false) {
        fclose($server);

        return ['error' => "client: {$errstr} ({$errno})"];
    }
    // Accept it and send a byte, so this one is genuinely readable.
    $peer = stream_socket_accept($server, 3.0);
    fwrite($peer, 'x');

    $silent = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    if ($silent === false) {
        fclose($ready);
        fclose($peer);
        fclose($server);

        return ['error' => "second client: {$errstr} ({$errno})"];
    }

    // One readable, one silent: the count is 1 and the read array is rewritten
    // down to the readable one.
    $read = [$ready, $silent];
    $write = null;
    $except = null;
    $n = stream_select($read, $write, $except, 1);
    $picked = count($read) === 1 && $read[array_key_first($read)] === $ready;

    // Nothing readable at all: the count is 0, the array is emptied, and the
    // call waits out its full second.
    $onlySilent = [$silent];
    $t0 = microtime(true);
    $none = stream_select($onlySilent, $write, $except, 1);
    $elapsed = microtime(true) - $t0;

    fclose($silent);
    fclose($ready);
    fclose($peer);
    fclose($server);

    return [
        'error' => null,
        'n' => $n,
        'picked' => $picked,
        'none' => $none,
        'emptied' => $onlySilent === [],
        'elapsed' => $elapsed,
    ];
};

$native = $probe();
$hooked = oxphp_async_await(oxphp_async($probe), 5.0);

foreach (['native' => $native, 'hooked' => $hooked] as $label => $r) {
    $t->assertNull("{$label}: the probe set up its sockets", $r['error']);
    $t->assertSame("{$label}: one stream reported readable", $r['n'], 1);
    $t->assertTrue("{$label}: the read array was rewritten to the readable stream", $r['picked']);
    $t->assertSame("{$label}: a wait with nothing readable returned zero", $r['none'], 0);
    $t->assertTrue("{$label}: the read array was emptied", $r['emptied']);
    $t->assertGreaterThan("{$label}: it waited out its 1s timeout", $r['elapsed'], 0.9);
    // Catches the hook waiting the timeout once and the delegate waiting it
    // again, which would land at 2.0s.
    $t->assertLessThan("{$label}: it did not wait its timeout twice", $r['elapsed'], 1.8);
}

$t->assertSame('hooked count matches the native one', $hooked['n'], $native['n']);
$t->assertSame('hooked empty-wait result matches the native one', $hooked['none'], $native['none']);

$t->done();
