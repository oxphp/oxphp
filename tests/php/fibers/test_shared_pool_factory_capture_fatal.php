<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A Shared\Pool's teardown releases its factory and $destroy callables, and
// releasing one releases what it captured: a destructor runs there. A fatal in
// it has to end the request as it would anywhere else, with the teardown run to
// its end on the fatal's way out — the pool leaves the shared registry.

$t = new TestCase('shared_pool_factory_capture_fatal', 'fibers');

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shared_closure_fatal.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked (this profile hooks streams): the read parks this request's fiber,
    // which leaves the worker free to serve the request just sent.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

/**
 * Entries the shared registry still counts but no longer lists: an entry is
 * listed while something holds it, and counted until its teardown has run to
 * its end. Null when the summary cannot be read.
 */
$unfinished = static function (): ?int {
    $summary = json_decode((string) file_get_contents('http://127.0.0.1:9090/__ox_shared/summary'), true);
    if (!is_int($summary['total_entries'] ?? null)) {
        return null;
    }
    $listed = 0;
    foreach ($summary['by_type'] ?? [] as $type) {
        $listed += $type['count'];
    }

    return $summary['total_entries'] - $listed;
};

$before = $unfinished();

$resp = $get('case=pool_drop_factory_capture');
$t->assertMatch('the fatal ended the inner request with a 500', $resp, '#^HTTP/\S+ 500 #');
$t->assertNotContains('nothing past the fatal ran', $resp, 'NOT-REACHED');

// A worker retires after three requests in a row end in a bailout. A request
// that completes in between keeps this one in service.
$t->assertMatch('a request after it completes', $get('case=none'), '#^HTTP/\S+ 200 #');

$after = $unfinished();
$t->assertSame(
    "the pool's teardown ran to its end",
    $before === null || $after === null ? null : $after - $before,
    0
);

$t->done();
