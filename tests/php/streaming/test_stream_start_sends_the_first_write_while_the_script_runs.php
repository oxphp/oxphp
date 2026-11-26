<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/stream_probe.php';

// A response started with oxphp_stream_start() reaches its client as it is
// written, not when its script ends, with the status and headers it set before
// its first write.
//
// The inner request writes a line, then waits for this request to say it has
// that line before writing its last one, which says whether it heard back. A
// response held until its script ends cannot be answered in time: the inner
// request waits out its three seconds. The same request started by its
// Content-Type, as an event stream is, is the control — it streams whether or
// not oxphp_stream_start() exists. On HTTP/1.1 and, over cleartext, HTTP/2. The
// test is the stream's client, so the stream runs on another worker.

$t = new TestCase('stream_start_sends_the_first_write_while_the_script_runs', 'streaming');

$protocols = ['h1' => CURL_HTTP_VERSION_1_1, 'h2' => CURL_HTTP_VERSION_2_PRIOR_KNOWLEDGE];
$cases = ['sse' => ['sse=1', 'text/event-stream'], 'start' => ['start=1', 'text/csv']];

foreach ($cases as $case => [$query, $type]) {
    foreach ($protocols as $proto => $version) {
        $token = bin2hex(random_bytes(8));
        $marker = stream_probe_marker($token);
        $told = false;
        $r = stream_probe_fetch(
            "/tests/streaming/fixture_stream_start.php?$query&token=$token",
            $version,
            static function (string $body) use ($marker, &$told): void {
                if (!$told && str_contains($body, "a\n")) {
                    $told = touch($marker);
                }
            }
        );
        if (is_file($marker)) {
            unlink($marker);
        }

        $label = "$case over $proto";
        $t->assertSame("$label: no transfer error", $r['error'], '');
        $t->assertSame("$label: the first line reached the client while the script ran", $r['body'], "a\nb:acked\n");
        $t->assertSame("$label: with the status set before it", $r['status'], 202);
        $t->assertSame("$label: and a header set before it", $r['headers']['x-stream-start'] ?? null, '1');
        $t->assertTrue("$label: as $type", str_starts_with($r['headers']['content-type'] ?? '', $type));
        $t->assertFalse("$label: as a stream, with no Content-Length", isset($r['headers']['content-length']));
    }
}

$t->done();
