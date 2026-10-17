<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// A stream waiting for room is still ended by its time limit.
//
// The stream sets a two-second limit, and its client connects and reads
// nothing, so it spends nearly all of that limit inside one flush. A flush that
// waited without coming back to the engine would outlast the limit for as long
// as the client stays connected. Both edges are read: the limit is what ended
// it, which it cannot have reached early, and it was ended close to the limit
// rather than whenever something else came along.

$t = new TestCase('stream_backpressure_honours_time_limit', 'fibers');

OxphpStreamBackpressureProbe::reset();

$stream = stream_backpressure_open('mib=64&kib=64&limit=2');
$t->assertTrue('connected to the stream', $stream !== false);

$t->assertTrue('the stream is waiting in a flush', stream_backpressure_stalled());
$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 6.0);
fclose($stream);

$ran = OxphpStreamBackpressureProbe::$endedAt - OxphpStreamBackpressureProbe::$startedAt;

$t->assertTrue('the stream ended while its client was still connected', $ended);
$t->assertSame('by its time limit', OxphpStreamBackpressureProbe::$connectionStatus & CONNECTION_TIMEOUT, CONNECTION_TIMEOUT);
$t->assertContains('as the limit reports itself', (string) OxphpStreamBackpressureProbe::$lastError, 'Maximum execution time');
$t->assertSame('from inside the flush it was waiting in', OxphpStreamBackpressureProbe::$stage, 'flushing');
$t->assertGreaterThan('not before the limit (ran ' . round($ran, 3) . ' s)', $ran, 1.95);
$t->assertLessThan('and not long after it', $ran, 3.0);

$t->done();
