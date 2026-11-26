<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/stream_probe.php';

// What a stream writes after its last flush reaches its client when the script
// ends, as under PHP-FPM, rather than being dropped with the request's buffers.
// The test is the stream's client, so the stream runs on another worker.

$t = new TestCase('stream_tail_after_the_last_flush_arrives', 'streaming');

$r = stream_probe_fetch('/tests/streaming/fixture_stream_tail.php', CURL_HTTP_VERSION_1_1);

$t->assertSame('no transfer error', $r['error'], '');
$t->assertSame('a stream', $r['headers']['transfer-encoding'] ?? null, 'chunked');
$t->assertSame('whose tail arrived', $r['body'], 'HEAD1TAIL');

$t->done();
