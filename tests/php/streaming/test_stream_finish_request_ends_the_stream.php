<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/stream_probe.php';

// A stream that calls oxphp_finish_request() ends there for its client, with
// the output it had not flushed yet, while its script goes on — whether it has
// flushed before or only called oxphp_stream_start(), in which case the call
// is its first flush and sends its headers too. oxphp_stream_start() after it
// returns false.
//
// The inner request calls oxphp_finish_request() and then waits for this
// request to say the response has ended, and records whether it heard back. A
// stream left open until its script ends cannot be answered in time: the inner
// request waits out its three seconds. On HTTP/1.1 and, over cleartext, HTTP/2.
// The test is the stream's client, so the stream runs on another worker.

$t = new TestCase('stream_finish_request_ends_the_stream', 'streaming');

$protocols = ['h1' => CURL_HTTP_VERSION_1_1, 'h2' => CURL_HTTP_VERSION_2_PRIOR_KNOWLEDGE];

foreach (['flush', 'start'] as $mode) {
    foreach ($protocols as $proto => $version) {
        $token = bin2hex(random_bytes(8));
        $marker = stream_probe_marker($token);
        $r = stream_probe_fetch("/tests/streaming/fixture_stream_finish.php?mode=$mode&token=$token", $version);
        touch($marker);

        $state = null;
        if (stream_probe_wait_for("$marker.state", 6.0)) {
            $state = json_decode((string) file_get_contents("$marker.state"), true);
            unlink("$marker.state");
        }
        unlink($marker);

        $label = "$mode over $proto";
        $t->assertSame("$label: no transfer error", $r['error'], '');
        $t->assertSame("$label: the response carries what was written before the call", $r['body'], "a\nb\n");
        $t->assertFalse("$label: as a stream, with no Content-Length", isset($r['headers']['content-length']));
        $t->assertSame("$label: and ended while the script was still running", $state['acked'] ?? null, true);
        $t->assertSame("$label: oxphp_stream_start() after it returns false", $state['late_start'] ?? null, false);
    }
}

$t->done();
