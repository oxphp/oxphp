<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A client that hangs up ends its request at a write, and nowhere earlier.
// Ending it where it stands — at whatever opcode it happens to be running — can
// leave a persistent connection (pfsockopen, a persistent Redis client)
// between a command and its reply, and every later request on that thread then
// reads the reply meant for this one.
//
// Three shapes:
//   plain  — an ordinary response. The server notices the client leaving while
//            the request sleeps; the request must run on to its next write and
//            be ended there.
//   stream — a stream that has not sent its headers yet. Noticed the same way,
//            and ended the same way.
//   open   — a stream that has sent its headers. Its client leaving is found out
//            only by a flush, and that flush — the write that found it — is
//            where it ends, not one pass of its loop later.

$mode = $_GET['mode'] ?? 'plain';
if (!in_array($mode, ['plain', 'stream', 'open'], true)) {
    $mode = 'plain';
}

// Written right before the write expected to end the request.
$beforeWrite = "/tmp/oxphp-cancel-ends-at-write-{$mode}.before";
// Written right after it: the request was not ended there.
$afterWrite = "/tmp/oxphp-cancel-ends-at-write-{$mode}.after";

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    @unlink($beforeWrite);
    @unlink($afterWrite);

    if ($mode !== 'plain') {
        header('Content-Type: text/event-stream');
    }
    if ($mode === 'open') {
        echo "data: prefill\n\n";
        oxphp_stream_flush();
    }

    // The client gives up at 0.3 s (curl --max-time).
    sleep(1);

    // What the request knew here is the premise. For plain and stream the
    // server has noticed by now and raised the interrupt, so reaching this line
    // at all means the opcodes in between did not end the request — and a
    // marker written before the server had noticed would prove nothing, so it
    // records connection_aborted(). An open stream has not been told yet: its
    // flush below is what finds out.
    @file_put_contents($beforeWrite, (string) connection_aborted());

    echo "data: after\n\n";
    if ($mode === 'plain') {
        flush();
    } else {
        oxphp_stream_flush();
    }

    @file_put_contents($afterWrite, 'not ended at the write');
    exit;
}

// action=check — the trigger finishes (or is ended) about 1 s in.
sleep(2);

$test = new TestCase("client_abort_ends_at_next_write_{$mode}", 'cancel');

$before = is_file($beforeWrite) ? file_get_contents($beforeWrite) : null;
$after = is_file($afterWrite);
if ($before !== null) {
    unlink($beforeWrite);
}
if ($after) {
    unlink($afterWrite);
}

$test->assertTrue('the request ran on up to the write', $before !== null);
$test->assertSame(
    'and knew there whether the client was gone',
    $before,
    $mode === 'open' ? '0' : '1'
);
$test->assertTrue('and was ended at that write', !$after);
$test->done();
