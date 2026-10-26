<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() on a POST parses a body the request has already parsed
// once: a SAPI reads a POST body before the script runs unless
// enable_post_data_reading is off, and the function reads it again
// unconditionally. The second read buffers a body of its own and
// puts it where the first one stood without closing it. In every other SAPI the
// destruction of the request's resource list closes the one it replaced; a
// worker's request has no such list, so the end of the request has to close
// both bodies itself, or every such request leaves one behind for the life of
// the worker.
//
// The replaced body must still outlive the call: a php://input handle opened
// before it reads that body, and reads it to the end of the request as it does
// under php-fpm. When the request ends that handle is closed with the body, as a
// handle on the current one is, rather than left naming it.
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile); the count
// assertion compares the second and third, since the first also pays for
// whatever the request path allocates once. Counts and the handle live in
// $sharedState — the worker-scope store the worker fixture keeps — because both
// only mean anything within one worker lifetime.

$t = new TestCase('request_parse_body_streams_bounded', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$t->assertSame('$_POST came from the body', $_POST['probe'] ?? null, 'reparse');

if ($sighting > 1) {
    $t->assertKeyExists(
        'the request before this one left its early handle in worker-scope state',
        $sharedState,
        'request_parse_body_early_handle'
    );
    // Asked about rather than read from: a build that closes the replaced body
    // without the handle leaves it naming freed memory.
    $t->assertFalse(
        'a handle on the replaced body is closed with its request, not left dangling',
        is_resource($sharedState['request_parse_body_early_handle'] ?? null)
    );
    unset($sharedState['request_parse_body_early_handle']);
}

$early = fopen('php://input', 'r');

$before = count(get_resources('stream'));
$parsed = request_parse_body();
$created = count(get_resources('stream')) - $before;

$t->assertSame('the second parse finds the body already read, as under php-fpm', $parsed, [[], []]);
// The premise: without a body of its own from the second parse there is nothing
// replaced, and the counts below would stay flat on any build.
$t->meta('streams_created_by_the_second_parse', $created);
$t->assertGreaterThan('and buffers a body of its own', $created, 0);

$t->assertSame(
    'a handle opened before the call still reads the replaced body',
    stream_get_contents($early),
    'probe=reparse'
);

$counts = $sighting > 1 ? (array) ($sharedState['request_parse_body_stream_counts'] ?? []) : [];
$counts[] = count(get_resources('stream'));
$sharedState['request_parse_body_stream_counts'] = $counts;

$t->meta('stream_counts', $counts);

if ($sighting < 3) {
    $sharedState['request_parse_body_early_handle'] = $early;
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    fclose($early);
    unset($sharedState['request_parse_body_stream_counts']);
    $t->assertCount('all three sightings landed on this worker', $counts, 3);
    $t->assertSame(
        'open stream count does not grow from one request_parse_body() POST to the next',
        $counts[2] ?? null,
        $counts[1] ?? null
    );
}

$t->done();
