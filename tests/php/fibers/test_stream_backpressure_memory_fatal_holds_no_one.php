<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// A stream that runs out of memory_limit while its client is not reading ends
// at that error, rather than waiting for room to flush it.
//
// PHP displays that error with the limit set aside, on a heap every request on
// the thread allocates from. A flush that waited there for room — the stream
// flushes implicitly, so the error is flushed as it is written — would leave the
// other requests on the thread allocating with no limit until the client read.
// The error is still delivered: it follows what was queued before it once the
// client reads again.

$t = new TestCase('stream_backpressure_memory_fatal_holds_no_one', 'fibers');

OxphpStreamBackpressureProbe::reset();

$sock = stream_backpressure_open('mib=512&kib=1024&memfatal=1');
$t->assertTrue('connected to the stream', $sock !== false);
$t->assertTrue('the stream is waiting in a flush', stream_backpressure_stalled());

// It runs out of memory once that flush returns. Reading frees the room for it,
// and the queue is full again by the time the error is flushed.
OxphpStreamBackpressureProbe::$command = 'fatal';
$deadline = microtime(true) + 10.0;
while (OxphpStreamBackpressureProbe::$stage === 'flushing' && microtime(true) < $deadline) {
    fread($sock, 65536);
    usleep(2000);
}
$t->assertSame('the stream went on to run out of memory', OxphpStreamBackpressureProbe::$stage, 'fatal');

$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 1.0);
$t->assertTrue('it ended at that error, its client still not reading', $ended);
$t->assertContains('which was memory_limit', (string) OxphpStreamBackpressureProbe::$lastError, 'Allowed memory size');

$body = fiber_inner_request('/tests/fibers/fixture_stream_backpressure.php?alloc=256', 10.0);
$t->assertFalse('the next request on the thread cannot go past memory_limit', str_starts_with($body, 'allocated'));
$t->assertContains('which stops it', (string) OxphpStreamBackpressureProbe::$allocError, 'Allowed memory size');

stream_set_timeout($sock, 10);
$deadline = microtime(true) + 10.0;
$tail = '';
while (!feof($sock) && microtime(true) < $deadline) {
    $piece = fread($sock, 1048576);
    if ($piece === false || ($piece === '' && stream_get_meta_data($sock)['timed_out'])) {
        break;
    }
    $tail = substr($tail . $piece, -8192);
}
$t->assertTrue('the client got to the end of the stream', feof($sock));
fclose($sock);
$t->assertContains('and the error came last', $tail, 'Allowed memory size');

$t->done();
