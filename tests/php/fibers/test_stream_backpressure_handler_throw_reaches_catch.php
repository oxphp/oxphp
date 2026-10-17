<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// What an output handler throws in a stream's flush reaches the script's catch
// when the stream's queue is full, without the client reading.
//
// PHP hands the stream what the failed handler was given, and flushes it with
// the exception still pending. Zend does not carry a pending exception across a
// fiber switch, so a flush that parked there for room would come back with it
// gone, and the script would carry on as though the handler had returned. A
// flush that waited for room on the thread instead would keep it, and hold the
// thread until the client read — the client is this request, on that thread.
// What the handlers were given is not lost: it follows what was queued before it
// once the client reads.
//
// Every chunk of this stream goes through a handler of its own that throws, and
// the stream is larger than its queue and the socket can hold, so the flushes
// made once the queue is full are of those.

$t = new TestCase('stream_backpressure_handler_throw_reaches_catch', 'fibers');

OxphpStreamBackpressureProbe::reset();

$mib = 96;
$kib = 512;
$chunks = intdiv($mib * 1024, $kib);

// The most the kernel lets a socket buffer: its send side and its receive side.
$most = static fn (string $name): int => (int) (preg_split(
    '/\s+/',
    trim((string) file_get_contents("/proc/sys/net/ipv4/$name")),
)[2] ?? 0);
$holds = 66 * $kib * 1024 + $most('tcp_wmem') + $most('tcp_rmem');
$t->assertGreaterThan(
    'the stream is larger than its queue and the socket can hold (' . intdiv($holds, 1048576) . ' MiB)',
    $mib * 1024 * 1024,
    $holds,
);

// A time limit, so that a flush that holds the thread is ended by it instead of
// wedging the profile's only worker.
$sock = stream_backpressure_open("mib=$mib&kib=$kib&throw=1&limit=5");
$t->assertTrue('connected to the stream', $sock !== false);

$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 4.0);
$t->assertTrue('the stream ended, its client still not reading', $ended);
$t->assertSame('every chunk went through a handler that threw', OxphpStreamBackpressureProbe::$thrown, $chunks);
$t->assertSame('and every exception reached the catch around its flush', OxphpStreamBackpressureProbe::$caught, $chunks);
$t->assertNull('with no error', OxphpStreamBackpressureProbe::$lastError);

$expected = hash_init('crc32b');
for ($i = 0; $i < $chunks; $i++) {
    hash_update($expected, stream_backpressure_chunk($i, $kib * 1024));
}
$expected = hash_final($expected);

stream_set_timeout($sock, 10);
$head = '';
while (!str_contains($head, "\r\n\r\n") && !feof($sock)) {
    $head .= (string) fread($sock, 1);
}
$received = 0;
$actual = hash_init('crc32b');
while (!feof($sock)) {
    $piece = fread($sock, 262144);
    if ($piece === false || ($piece === '' && stream_get_meta_data($sock)['timed_out'])) {
        break;
    }
    hash_update($actual, $piece);
    $received += strlen($piece);
}
fclose($sock);

$t->assertSame('the client got every chunk', $received, $mib * 1024 * 1024);
$t->assertSame('whole and in order', hash_final($actual), $expected);

$t->done();
