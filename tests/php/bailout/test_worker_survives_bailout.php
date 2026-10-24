<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Runs right after test_headers_bailout.php. Two jobs.
//
// First, confirm the trap actually fired where it was aimed: the marker the
// previous request left behind has to record a memory_limit fatal on the
// headers() call, not somewhere earlier in userland. Without that the run
// proves nothing — the window this test exists to cover was never entered.
// From PHP 8.4.27 / 8.5.12 it is not entered even then; see below.
//
// Second, show the server is still there and its per-request state is still
// usable. Up to PHP 8.4.26 / 8.5.11, where the fatal unwinds out of the walk
// by longjmp, state left behind by that longjmp would have aborted the
// process, so on a server carrying the defect this request is answered by
// nobody. Later releases raise the fatal once the walk has returned (see
// test_headers_bailout.php), and there this request checks only that the
// thread that took the fatal goes on serving and can read its request state.

$t = new TestCase('worker_survives_bailout', 'bailout');

$marker = @file_get_contents('/tmp/oxphp-bailout-marker.json');
$t->assertNotEqual('the bailout request left its marker', $marker, false);

$recorded = json_decode((string) $marker, true);
$error = $recorded['error'] ?? null;

$t->assertNotNull('the marker records a fatal', $error);
$t->assertContains(
    'the fatal is a memory_limit fatal',
    (string) ($error['message'] ?? ''),
    'Allowed memory size'
);
$t->assertSame(
    'the fatal landed on the headers() call',
    $error['line'] ?? -1,
    $recorded['expected_line'] ?? -2
);

// The line alone cannot tell a fatal raised inside the walk from one raised in
// the method around it, so pin the allocation that failed as well: only the
// copy of the padding header is anywhere near this size, and that copy happens
// one entry at a time inside the callback under test.
//
// That holds up to PHP 8.4.26 / 8.5.11. From 8.4.27 / 8.5.12 an allocation
// served from a fresh chunk reports the chunk's size, whatever it asked for:
// the number then says only that the limit was crossed by an allocation that
// did not fit in what was left of the mapped memory, and the line is all that
// places it on the headers() call. Those releases are the first to know
// memory_hard_limit_percent, which is what tells them apart here.
preg_match('/tried to allocate (\d+) bytes/', (string) ($error['message'] ?? ''), $m);
$tried = (int) ($m[1] ?? 0);
if (ini_get('memory_hard_limit_percent') === false) {
    $expected = (int) ($recorded['expected_bytes'] ?? 0);
    $t->assertTrue(
        "the failed allocation is the padding header copy (tried {$tried}, header {$expected})",
        $expected > 0 && $tried >= $expected && $tried < $expected + 8192
    );
} else {
    $chunk = (int) ($recorded['chunk_bytes'] ?? 0);
    $t->assertTrue(
        "the allocation past the limit took a fresh chunk (tried {$tried}, chunk {$chunk})",
        $chunk > 0 && $tried === $chunk
    );
}

// php://input reads the same per-request state, and PHP reads it once more
// from request shutdown on every request.
$t->assertSame('php://input is readable', file_get_contents('php://input'), '');

// headers(), query() and cookies() walk that state through the pair-visiting
// callbacks, which call back into PHP for every entry.
$request = oxphp_http_request();
$t->assertNotEmpty('headers() is not empty', $request->headers());
$t->assertTrue('query() returns array', is_array($request->query()));
$t->assertTrue('cookies() returns array', is_array($request->cookies()));

$t->done();
