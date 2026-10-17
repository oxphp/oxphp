<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// What is left of a stream when it is ended while its client is not reading
// does not hold the worker, and still reaches the client once it reads.
//
// The stream's time limit ends it inside a flush, with its queue full. The
// chunk that flush was holding is left for the end of the request to hand to the
// connection, with the queue still full. Handed over by waiting for room, it
// would hold the worker thread until the client read — and the client is this
// request, on that thread.

$t = new TestCase('stream_backpressure_end_leaves_the_worker_free', 'fibers');

OxphpStreamBackpressureProbe::reset();

$kib = 64;
$sock = stream_backpressure_open("mib=64&kib=$kib&limit=1");
$t->assertTrue('connected to the stream', $sock !== false);

$t->assertTrue('the stream is waiting in a flush', stream_backpressure_stalled());
$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 4.0);
$t->assertTrue('its time limit ended it', $ended);
$t->assertSame('the limit ended it inside that flush', OxphpStreamBackpressureProbe::$stage, 'flushing');

// The chunks before the one that flush held went through the queue.
$held = OxphpStreamBackpressureProbe::$flushes;

$before = microtime(true);
$pong = fiber_inner_request('/tests/fibers/fixture_stream_backpressure.php?ping=1', 5.0);
$t->assertSame('the worker serves the next request', $pong, 'pong');
$t->assertLessThan('promptly', microtime(true) - $before, 1.0);

$expected = hash_init('crc32b');
for ($i = 0; $i <= $held; $i++) {
    hash_update($expected, stream_backpressure_chunk($i, $kib * 1024));
}
$expected = hash_final($expected);

stream_set_timeout($sock, 10);
$head = '';
while (!str_contains($head, "\r\n\r\n") && !feof($sock)) {
    $head .= (string) fread($sock, 1);
}

// The chunks are hashed as they arrive; the error the limit raised may follow
// them.
$prefix = ($held + 1) * $kib * 1024;
$received = 0;
$actual = hash_init('crc32b');
while (!feof($sock)) {
    $piece = fread($sock, 262144);
    if ($piece === false || ($piece === '' && stream_get_meta_data($sock)['timed_out'])) {
        break;
    }
    if ($received < $prefix) {
        hash_update($actual, substr($piece, 0, $prefix - $received));
    }
    $received += strlen($piece);
}
fclose($sock);

$t->assertGreaterThan('the client got past every chunk up to the held one', $received, $prefix - 1);
$t->assertSame('which arrived whole and in order', hash_final($actual), $expected);

$t->done();
