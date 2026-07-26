<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_declines', 'hooks');

// Three shapes the hook must hand to the native handler untouched, each verified
// against what native itself does with them: the probe runs once in the request
// context (no fiber, so the hook delegates on its first line) and once inside an
// async task (where the hook is otherwise active). Any divergence between the
// two columns is the hook changing behaviour it promised not to touch.
$probe = static function (): array {
    // 1. Buffered data. stream_select() answers from the stream's own buffer
    //    without looking at any descriptor, so a hook that parked instead would
    //    sleep through data the caller already holds.
    $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    $addr = stream_socket_get_name($server, false);
    $client = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
    $peer = stream_socket_accept($server, 3.0);
    fwrite($peer, 'abcdef');
    // Pull the bytes into the stream's read buffer, leaving some unread.
    stream_set_timeout($client, 2);
    fread($client, 2);

    $read = [$client];
    $write = null;
    $except = null;
    $t0 = microtime(true);
    $buffered = stream_select($read, $write, $except, 2);
    $bufferedElapsed = microtime(true) - $t0;

    fclose($client);
    fclose($peer);
    fclose($server);

    // The two descriptorless cases below are supposed to emit native's own
    // diagnostic, and the diagnostic is half of what they assert. Capture it
    // instead of suppressing it: the harness turns every warning into an
    // exception regardless of the @ operator, and a swallowed warning is exactly
    // the kind of behaviour change this test exists to catch.
    $warnings = [];
    set_error_handler(static function (int $no, string $str) use (&$warnings): bool {
        $warnings[] = $str;

        return true;
    });

    // 2. Nothing but a stream with no descriptor. Native finds no selectable
    //    descriptor at all and raises the error it raises for a caller who
    //    passed no streams — before any waiting and before the buffered-data
    //    shortcut it would otherwise have answered from.
    $mem = fopen('php://memory', 'r+');
    fwrite($mem, 'hello');
    rewind($mem);
    $read2 = [$mem];
    $onlyError = null;
    $t1 = microtime(true);
    try {
        stream_select($read2, $write, $except, 1);
    } catch (\Throwable $ex) {
        $onlyError = $ex::class . ': ' . $ex->getMessage();
    }
    $onlyElapsed = microtime(true) - $t1;
    fclose($mem);

    // 3. The same descriptorless stream alongside a readable socket. Native
    //    warns, drops it, and selects on the remainder — a sequence with its own
    //    warning text and its own array rewriting, none of which is the hook's
    //    to reproduce.
    $server2 = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
    $addr2 = stream_socket_get_name($server2, false);
    $client2 = stream_socket_client("tcp://{$addr2}", $errno, $errstr, 3.0);
    $peer2 = stream_socket_accept($server2, 3.0);
    fwrite($peer2, 'y');

    $mem2 = fopen('php://memory', 'r+');
    $read3 = [$mem2, $client2];
    $t2 = microtime(true);
    $mixed = stream_select($read3, $write, $except, 2);
    $mixedElapsed = microtime(true) - $t2;
    $mixedKept = count($read3) === 1 && $read3[array_key_first($read3)] === $client2;

    restore_error_handler();

    fclose($mem2);
    fclose($client2);
    fclose($peer2);
    fclose($server2);

    return [
        'buffered' => $buffered,
        'bufferedElapsed' => $bufferedElapsed,
        'onlyError' => $onlyError,
        'onlyElapsed' => $onlyElapsed,
        'mixed' => $mixed,
        'mixedElapsed' => $mixedElapsed,
        'mixedKept' => $mixedKept,
        'warned' => count(array_filter(
            $warnings,
            static fn (string $w): bool => str_contains($w, 'as a select()able descriptor')
        )) > 0,
    ];
};

$native = $probe();
$hooked = oxphp_async_await(oxphp_async($probe), 10.0);

foreach (['native' => $native, 'hooked' => $hooked] as $label => $r) {
    $t->assertSame("{$label}: the buffered stream was reported readable", $r['buffered'], 1);
    $t->assertLessThan("{$label}: the buffered case answered immediately", $r['bufferedElapsed'], 0.5);
    $t->assertContains(
        "{$label}: a descriptorless stream alone raised the native error",
        (string) $r['onlyError'],
        'ValueError: No stream arrays were passed'
    );
    $t->assertLessThan("{$label}: the descriptorless case did not wait", $r['onlyElapsed'], 0.5);
    $t->assertSame("{$label}: the socket beside it was reported readable", $r['mixed'], 1);
    $t->assertTrue("{$label}: the array kept only the socket", $r['mixedKept']);
    $t->assertLessThan("{$label}: the mixed case answered immediately", $r['mixedElapsed'], 0.5);
    $t->assertTrue("{$label}: native's own descriptor warning still reached the caller", $r['warned']);
}

$t->assertSame('hooked buffered result matches the native one', $hooked['buffered'], $native['buffered']);
$t->assertSame('hooked mixed result matches the native one', $hooked['mixed'], $native['mixed']);
$t->assertSame('hooked error matches the native one', $hooked['onlyError'], $native['onlyError']);

$t->done();
