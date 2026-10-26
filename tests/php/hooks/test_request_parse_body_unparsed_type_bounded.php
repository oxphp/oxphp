<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() on a POST of a type PHP has no parser for — JSON here.
// The parse before the script runs buffers such a body all the same, and so
// does the call, before it throws for the type: the body it buffers replaces
// the first one. The end of the request has to close both, as it does for a
// urlencoded body (hooks/test_request_parse_body_streams_bounded).
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile); the assertion
// compares the second and third.

$t = new TestCase('request_parse_body_unparsed_type_bounded', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$t->assertSame('the body came through', file_get_contents('php://input'), '{"probe":"json"}');

$before = count(get_resources('stream'));
$thrown = null;
try {
    request_parse_body();
} catch (\RequestParseBodyException $e) {
    $thrown = $e->getMessage();
}
$created = count(get_resources('stream')) - $before;

$t->assertSame(
    'the call refuses the type, as PHP does',
    $thrown,
    'Content-Type "application/json" is not supported'
);
// The premise: the call buffered a body of its own before refusing; without it
// the counts below stay flat on any build.
$t->meta('streams_created_by_the_call', $created);
$t->assertGreaterThan('and buffers a body of its own first', $created, 0);

$counts = $sighting > 1 ? (array) ($sharedState['request_parse_body_unparsed_counts'] ?? []) : [];
$counts[] = count(get_resources('stream'));
$sharedState['request_parse_body_unparsed_counts'] = $counts;

$t->meta('stream_counts', $counts);

if ($sighting < 3) {
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    unset($sharedState['request_parse_body_unparsed_counts']);
    $t->assertCount('all three sightings landed on this worker', $counts, 3);
    $t->assertSame(
        'open stream count does not grow from one such request to the next',
        $counts[2] ?? null,
        $counts[1] ?? null
    );
}

$t->done();
