<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/stream_backpressure_probe.php';

// A stream whose client has stopped reading waits for room without holding the
// worker: another request on the same thread is served meanwhile, and the
// stream still ends when its client goes.
//
// The stream's client here connects and reads nothing. Its queue fills, and its
// flush has to wait. The premise is read where it is used, before and after the
// other request: the stream is inside a flush, and no flush of it has returned
// across the reading. Read only before, it would be satisfied by a stream that
// was stuck at the time and then moved on.

$t = new TestCase('stream_backpressure_leaves_the_worker_free', 'fibers');

OxphpStreamBackpressureProbe::reset();

$stream = stream_backpressure_open('mib=64&kib=64');
$t->assertTrue('connected to the stream', $stream !== false);

$stalled = stream_backpressure_stalled();
$t->assertTrue('the stream is waiting in a flush', $stalled);
$flushes = OxphpStreamBackpressureProbe::$flushes;

$before = microtime(true);
$pong = fiber_inner_request('/tests/fibers/fixture_stream_backpressure.php?ping=1', 5.0);
$took = microtime(true) - $before;

$t->assertSame('another request on the same worker is answered', $pong, 'pong');
$t->assertLessThan('and promptly', $took, 1.0);
$t->assertSame('while the stream was still in that flush', OxphpStreamBackpressureProbe::$stage, 'flushing');
$t->assertSame('and none of its flushes returned meanwhile', OxphpStreamBackpressureProbe::$flushes, $flushes);
$t->assertFalse('and it had not ended', OxphpStreamBackpressureProbe::$ended);

// Its client goes. A stream that did not ask to outlive its client is ended at
// the flush that finds it gone, and the flush waiting for room is that flush.
fclose($stream);
$ended = stream_backpressure_wait(static fn (): bool => OxphpStreamBackpressureProbe::$ended, 3.0);

$t->assertTrue('the stream ended once its client was gone', $ended);
$t->assertSame('as a client that left', OxphpStreamBackpressureProbe::$connectionStatus & CONNECTION_ABORTED, CONNECTION_ABORTED);
$t->assertSame('not by its time limit', OxphpStreamBackpressureProbe::$connectionStatus & CONNECTION_TIMEOUT, 0);
$t->assertSame('from inside that flush', OxphpStreamBackpressureProbe::$stage, 'flushing');

$t->done();
