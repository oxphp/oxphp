<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The disconnect idiom the SSE guide describes — a `while (!connection_aborted())`
// loop around a flush — run against a client that hangs up, once as it is and
// once after ignore_user_abort(true).
//
// A stream that has sent its headers learns that its client has gone only when
// it flushes. As it is, that flush is where the script is stopped: no code after
// it runs, so the loop condition is never evaluated with the disconnect in it and
// `finally` does not run — while the shutdown functions and destructors that run
// at the end of any request still do, each group up to the first one that writes
// output: that write is stopped as well, and the rest of its group does not run. After
// ignore_user_abort(true) the flush returns, connection_aborted() reports the
// disconnect from then on, the loop leaves through its condition and runs its
// `finally`, and every shutdown function and destructor runs to its end.
//
// Markers the trigger writes and the check reads:
//   before          — "<flush number>:<connection_aborted()>", just before each flush
//   after           — the same, just after each flush returns
//   loop_exit       — how many flushes there were, written after the loop
//   finally         — written by the `finally` block
//   shutdown        — connection_aborted() as the first shutdown function sees it
//   shutdown_entered, shutdown_echoed — written by the second one, before and
//                     after it echoes
//   shutdown_next   — written by the third one
//   destruct, destruct_echoed, destruct_next — written by the destructors of three
//                     objects the script holds; the second echoes first, and
//                     writes destruct_entered before that

$ignore = ($_GET['ignore'] ?? '0') === '1';
$prefix = '/tmp/oxphp-sse-loop-exit-' . ($ignore ? 'ignored' : 'default');
$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // Not @unlink(): in worker mode an error handler a previous request on this
    // worker installed is still there, and it fires despite @.
    foreach (['before', 'after', 'loop_exit', 'finally', 'shutdown', 'shutdown_entered', 'shutdown_echoed', 'shutdown_next', 'destruct', 'destruct_entered', 'destruct_echoed', 'destruct_next'] as $name) {
        if (is_file("{$prefix}.{$name}")) {
            unlink("{$prefix}.{$name}");
        }
    }

    if ($ignore) {
        ignore_user_abort(true);
    }

    register_shutdown_function(static function () use ($prefix): void {
        file_put_contents("{$prefix}.shutdown", (string) connection_aborted());
    });
    register_shutdown_function(static function () use ($prefix): void {
        file_put_contents("{$prefix}.shutdown_entered", 'entered');
        echo "event: bye\n\n";
        file_put_contents("{$prefix}.shutdown_echoed", 'returned');
    });
    register_shutdown_function(static function () use ($prefix): void {
        file_put_contents("{$prefix}.shutdown_next", 'ran');
    });

    $destructs = static fn (string $name, bool $echo): object => new class ($prefix, $name, $echo) {
        public function __construct(private string $prefix, private string $name, private bool $echo)
        {
        }

        public function __destruct()
        {
            if ($this->echo) {
                file_put_contents("{$this->prefix}.destruct_entered", 'entered');
                echo "event: bye\n\n";
            }
            file_put_contents("{$this->prefix}.{$this->name}", 'destructed');
        }
    };
    $destructNext = $destructs('destruct_next', false);
    $destructEchoed = $destructs('destruct_echoed', true);
    $guard = $destructs('destruct', false);

    header('Content-Type: text/event-stream');

    // The client gives up at 0.3 s (curl --max-time): after the first flush, before
    // the second.
    $flushes = 0;
    try {
        while (!connection_aborted()) {
            echo "data: tick\n\n";
            file_put_contents("{$prefix}.before", ($flushes + 1) . ':' . connection_aborted());
            oxphp_stream_flush();
            $flushes++;
            file_put_contents("{$prefix}.after", $flushes . ':' . connection_aborted());
            sleep(1);
        }
        file_put_contents("{$prefix}.loop_exit", (string) $flushes);
    } finally {
        file_put_contents("{$prefix}.finally", 'ran');
    }
    return;
}

// action=check — the trigger is stopped about 1 s in, or ends about 2 s in.
sleep(3);

$read = static function (string $name) use ($prefix): ?string {
    $file = "{$prefix}.{$name}";
    if (!is_file($file)) {
        return null;
    }
    $content = file_get_contents($file);
    unlink($file);
    return $content === false ? null : $content;
};

$before = $read('before');
$after = $read('after');
$loopExit = $read('loop_exit');
$finally = $read('finally');
$shutdown = $read('shutdown');
$shutdownEntered = $read('shutdown_entered');
$shutdownEchoed = $read('shutdown_echoed');
$shutdownNext = $read('shutdown_next');
$destruct = $read('destruct');
$destructEntered = $read('destruct_entered');
$destructEchoed = $read('destruct_echoed');
$destructNext = $read('destruct_next');

$test = new TestCase('sse_loop_exit_on_disconnect_' . ($ignore ? 'ignored' : 'default'), 'cancel');
$test->meta('markers', compact('before', 'after', 'loopExit', 'finally', 'shutdown', 'shutdownEntered', 'shutdownEchoed', 'shutdownNext', 'destruct', 'destructEntered', 'destructEchoed', 'destructNext'));

// The premise, in both runs: the script got as far as the flush after its client
// left, and right up to that flush connection_aborted() still said the client was
// there — so whatever happens next is that flush's doing.
$test->assertSame('the second flush was reached with connection_aborted() still false', $before, '2:0');

if ($ignore) {
    $test->assertSame('that flush returned, and connection_aborted() reported the disconnect', $after, '2:1');
    $test->assertSame('the loop left through its condition, with no flush after it', $loopExit, '2');
    $test->assertSame('its finally ran', $finally, 'ran');
} else {
    $test->assertSame('no code after that flush ran', $after, '1:0');
    $test->assertNull('so the loop never left through its condition', $loopExit);
    $test->assertNull('and its finally did not run', $finally);
}

$test->assertSame('the shutdown function ran and saw connection_aborted()', $shutdown, '1');
// Both chains reach the member that writes output — whichever of them runs
// first, and that depends on the mode, the other is not cut short by it, as the
// two are separate — so what follows is about that write and not about a
// callback or destructor that never started.
$test->assertSame('the shutdown function that writes output was entered', $shutdownEntered, 'entered');
$test->assertSame('the destructor that writes output was entered', $destructEntered, 'entered');

if ($ignore) {
    $test->assertSame('a shutdown function that writes output ran to its end', $shutdownEchoed, 'returned');
    $test->assertSame('and so did the one registered after it', $shutdownNext, 'ran');
    $test->assertSame('every destructor ran', [$destruct, $destructEchoed, $destructNext], ['destructed', 'destructed', 'destructed']);
} else {
    // Unlike under PHP-FPM, which discards what is written once it has found the
    // client gone, a write here still reaches the server and is stopped there.
    $test->assertNull('a shutdown function that writes output stopped at that write', $shutdownEchoed);
    $test->assertNull('and the one registered after it did not run', $shutdownNext);
    $test->assertNull('a destructor that writes output stopped at that write', $destructEchoed);
    // Outside worker mode the script's objects go newest first, in worker mode
    // oldest first, so which of the other two comes before the one that writes
    // depends on the mode: one of them ran, and the one after it did not.
    $test->assertSame(
        'the destructor before it ran, and the one after it did not',
        count(array_filter([$destruct, $destructNext], static fn (?string $v): bool => $v === 'destructed')),
        1,
    );
}
$test->done();
