<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A fatal error raised while an await releases the promise's closure ends the
// request, and the await still finishes on its way out: the task's result,
// which keeps a Shared\Map alive until the script has it, is let go of, and
// the map leaves the shared registry with the request.

$t = new TestCase('promise_closure_release_fatal', 'async');

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/async/fixture_promise_closure_release_fatal.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

/** Entries the shared registry counts, or null when the summary cannot be read. */
$entries = static function (): ?int {
    $summary = json_decode((string) file_get_contents('http://127.0.0.1:9090/__ox_shared/summary'), true);

    return is_int($summary['total_entries'] ?? null) ? $summary['total_entries'] : null;
};

$before = $entries();

$resp = $get('case=await');
$t->assertMatch('the fatal ended the inner request with a 500', $resp, '#^HTTP/\S+ 500 #');
$t->assertNotContains('nothing past the fatal ran', $resp, 'NOT-REACHED');
$t->assertMatch('a request after it completes', $get('case=none'), '#^HTTP/\S+ 200 #');

$after = $entries();
$t->assertSame(
    "the task's result let go of the map",
    $before === null || $after === null ? null : $after - $before,
    0
);

// A promise never awaited has its closure released at the request's end, and
// releasing it runs a destructor that raises a fatal error. The fatal has to
// end that request, not wait on its thread to be raised in the next request
// there. This test's own request holds the other of the profile's two PHP
// threads, so every inner request lands on that one.
$run = bin2hex(random_bytes(4));
$get("case=shutdown&run=$run");
$t->assertTrue(
    "shutdown: the destructor ran at the request's end",
    file_exists(sys_get_temp_dir() . "/oxphp-promise-closure-shutdown-$run")
);
$resp = $get('case=async');
$t->assertMatch('shutdown: the next request on that thread completes', $resp, '#^HTTP/\S+ 200 #');
$t->assertContains('shutdown: its call into the server returns', $resp, 'ASYNC: 42');

$t->done();
