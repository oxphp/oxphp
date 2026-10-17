<?php

declare(strict_types=1);

// Inner request for the fibers/test_stream_backpressure_* tests.
//
// Streams ?mib= MiB in chunks of ?kib= KiB, one flush per chunk, as fast as it
// can. Its client decides how fast that is: a stream's flush hands the chunk to
// a bounded queue the connection drains, and once the queue is full the flush
// has to wait for room. Whether that wait holds the worker thread, and every
// other request on it, is what the tests read.
//
// ?limit= sets a time limit first, for the test that a wait for room still ends
// at it. ?ping=1 answers 'pong' and touches nothing, for the tests' reads of
// whether the worker is free to serve another request.
//
// ?ignore=1&after=N: calls ignore_user_abort(true), and once it sees its client
// gone makes N more flushes of 1 MiB each, measuring its resident memory around
// them, then stops.
//
// ?memfatal=1: once a test sets the probe's command to 'fatal', runs out of
// memory_limit after its next flush, with implicit flush on, so the error PHP
// displays is flushed to the stream as it is written. ?alloc=N allocates N MiB
// and answers 'allocated <bytes>', recording what it saw of the stream first.
//
// ?throw=1: every chunk goes through an output handler of its own that throws
// in the chunk's flush, and the flush's catch takes the handler off and counts
// what it caught.

if (isset($_GET['ping'])) {
    echo 'pong';
    return;
}

require_once __DIR__ . '/stream_backpressure_probe.php';

if (isset($_GET['alloc'])) {
    register_shutdown_function(static function (): void {
        OxphpStreamBackpressureProbe::$allocError = error_get_last()['message'] ?? null;
    });
    OxphpStreamBackpressureProbe::$allocStage = OxphpStreamBackpressureProbe::$stage;
    OxphpStreamBackpressureProbe::$allocEnded = OxphpStreamBackpressureProbe::$ended;
    $block = str_repeat('y', (int) $_GET['alloc'] * 1024 * 1024);
    echo 'allocated ', strlen($block);
    return;
}

OxphpStreamBackpressureProbe::reset();
OxphpStreamBackpressureProbe::$startedAt = microtime(true);

$memfatal = isset($_GET['memfatal']);
$throw = isset($_GET['throw']);

register_shutdown_function(static function () use ($memfatal): void {
    if ($memfatal) {
        // Not left to whatever the worker serves after this.
        ob_implicit_flush(false);
        ini_restore('display_errors');
    }
    OxphpStreamBackpressureProbe::$connectionStatus = connection_status();
    OxphpStreamBackpressureProbe::$lastError = error_get_last()['message'] ?? null;
    OxphpStreamBackpressureProbe::$endedAt = microtime(true);
    OxphpStreamBackpressureProbe::$ended = true;
});

if (isset($_GET['limit'])) {
    set_time_limit((int) $_GET['limit']);
}
if (isset($_GET['ignore'])) {
    ignore_user_abort(true);
}
if ($memfatal) {
    ini_set('display_errors', '1');
}
$after = (int) ($_GET['after'] ?? 0);

$size = (int) ($_GET['kib'] ?? 64) * 1024;
$chunks = intdiv((int) ($_GET['mib'] ?? 32) * 1024 * 1024, $size);

header('Content-Type: application/octet-stream');

$thrower = static function (): string {
    OxphpStreamBackpressureProbe::$stage = 'thrown';
    OxphpStreamBackpressureProbe::$thrown++;
    throw new RuntimeException('thrown by the output handler');
};

for ($i = 0; $i < $chunks; $i++) {
    if ($throw) {
        ob_start($thrower);
    }
    echo stream_backpressure_chunk($i, $size);
    OxphpStreamBackpressureProbe::$stage = 'flushing';
    try {
        oxphp_stream_flush();
    } catch (RuntimeException $e) {
        ob_end_clean();
        OxphpStreamBackpressureProbe::$caught++;
    }
    OxphpStreamBackpressureProbe::$stage = 'flushed';
    OxphpStreamBackpressureProbe::$flushes++;

    if ($after > 0 && connection_aborted() === 1) {
        // What it writes from here on has no one to go to.
        $block = str_repeat('z', 1024 * 1024);
        OxphpStreamBackpressureProbe::$rssBefore = stream_backpressure_rss();
        for ($j = 0; $j < $after; $j++) {
            echo $block;
            oxphp_stream_flush();
            OxphpStreamBackpressureProbe::$flushesAfterGone++;
        }
        OxphpStreamBackpressureProbe::$rssAfter = stream_backpressure_rss();
        break;
    }

    if ($memfatal && OxphpStreamBackpressureProbe::$command === 'fatal') {
        OxphpStreamBackpressureProbe::$stage = 'fatal';
        ob_implicit_flush(true);
        $block = str_repeat('x', 1 << 30);
    }
}

OxphpStreamBackpressureProbe::$stage = 'done';
