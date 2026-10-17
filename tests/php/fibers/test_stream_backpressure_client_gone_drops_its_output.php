<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// What a stream flushes once its client has gone is dropped, not kept.
//
// The stream ignores user abort, so it goes on after its client leaves while it
// is waiting in a flush, and flushes another 256 MiB, 1 MiB at a time. Kept,
// that output would grow the worker by all of it, for as long as the request
// ran.

$t = new TestCase('stream_backpressure_client_gone_drops_its_output', 'fibers');

OxphpStreamBackpressureProbe::reset();

$after = 256;
$sock = stream_backpressure_open("mib=64&kib=64&ignore=1&after=$after");
$t->assertTrue('connected to the stream', $sock !== false);
$t->assertTrue('the stream is waiting in a flush', stream_backpressure_stalled());

fclose($sock);

$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 10.0);
$t->assertTrue('the stream ran to its end', $ended);
$t->assertSame('it went on flushing after its client left', OxphpStreamBackpressureProbe::$flushesAfterGone, $after);
$t->assertSame('its connection reads as aborted', OxphpStreamBackpressureProbe::$connectionStatus, CONNECTION_ABORTED);
$t->assertNull('and nothing ended it early', OxphpStreamBackpressureProbe::$lastError);

$grown = OxphpStreamBackpressureProbe::$rssAfter - OxphpStreamBackpressureProbe::$rssBefore;
$t->assertTrue('its resident memory was measured', OxphpStreamBackpressureProbe::$rssBefore > 0);
$t->assertLessThan('without keeping what it flushed', $grown, 64 * 1024 * 1024);

$t->done();
