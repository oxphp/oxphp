<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// A stream larger than everything between it and its client can hold reaches
// that client whole, when the client is a request on the same worker thread.
//
// The stream flushes faster than it is read, so its queue fills and a flush has
// to wait for room. Room is made only by this request reading, and with one
// worker thread this request runs only when the stream's request is not
// running. A flush that waited by holding the thread would wait for a read that
// cannot happen until it returns: the two requests would wait on each other for
// as long as the process lives. A flush that waits by parking its request lets
// this one read.
//
// 32 MiB is past what the queue (64 chunks), the connection and the loopback
// socket buffers hold together, so the stream cannot simply write it all ahead
// of the read.

$t = new TestCase('stream_backpressure_delivers_whole_body', 'fibers');

OxphpStreamBackpressureProbe::reset();

$kib = 64;
$mib = 32;
$chunks = intdiv($mib * 1024, $kib);

$expected = hash_init('crc32b');
for ($i = 0; $i < $chunks; $i++) {
    hash_update($expected, stream_backpressure_chunk($i, $kib * 1024));
}
$expected = hash_final($expected);

// Read and hashed as it arrives rather than kept: the body alone is a quarter
// of the default memory_limit.
$sock = stream_backpressure_open("mib=$mib&kib=$kib");
$t->assertTrue('connected to the stream', $sock !== false);
stream_set_timeout($sock, 12);

$head = '';
while (!str_contains($head, "\r\n\r\n") && !feof($sock)) {
    $head .= (string) fread($sock, 1);
}
$t->assertContains('the stream answered 200', $head, ' 200 ');

$received = 0;
$actual = hash_init('crc32b');
while (!feof($sock)) {
    $piece = fread($sock, 262144);
    if ($piece === false || ($piece === '' && stream_get_meta_data($sock)['timed_out'])) {
        break;
    }
    $received += strlen($piece);
    hash_update($actual, $piece);
}
fclose($sock);

$t->assertSame('the whole body arrived', $received, $mib * 1024 * 1024);
$t->assertSame('in order and intact', hash_final($actual), $expected);
$t->assertSame('the stream ran to its end', OxphpStreamBackpressureProbe::$stage, 'done');

// And the worker is left serving.
$t->assertSame(
    'the worker answers the next request',
    fiber_inner_request('/tests/fibers/fixture_stream_backpressure.php?ping=1', 5.0),
    'pong'
);

$t->done();
