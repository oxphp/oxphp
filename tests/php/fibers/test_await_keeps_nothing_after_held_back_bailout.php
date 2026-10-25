<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A fatal error raised while an await releases a promise's closure ends the
// request, and the await stops there: it hands the script no result and
// releases no other promise's closure. The worker keeps its heap across
// requests, so what the await did after the fatal would stay in it, and a
// Shared\Map in it would stay in the shared registry until the worker exits.

$t = new TestCase('await_keeps_nothing_after_held_back_bailout', 'fibers');

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_held_back_bailout_await_result.php?$query HTTP/1.0\r\n"
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

foreach (['await', 'await_race', 'await_any_next'] as $case) {
    $before = $entries();

    $resp = $get("case=$case");
    $t->assertMatch("$case: the fatal ended the inner request with a 500", $resp, '#^HTTP/\S+ 500 #');
    $t->assertNotContains("$case: nothing past the fatal ran", $resp, 'NOT-REACHED');
    // A worker retires after three requests in a row end in a bailout.
    $t->assertMatch("$case: a request after it completes", $get('case=none'), '#^HTTP/\S+ 200 #');

    // The request's own promise cleanup releases what the await left, as the
    // request ends or on a scheduler tick after it.
    $after = $entries();
    for ($i = 0; $i < 30 && $before !== null && $after !== $before; $i++) {
        usleep(100_000);
        $after = $entries();
    }
    $t->assertSame(
        "$case: no Shared\\Map is left in the registry",
        $before === null || $after === null ? null : $after - $before,
        0
    );
}

$t->done();
