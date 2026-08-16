<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The second way a request gets a body, and the one the SAPI never sees. On a
// multipart POST the engine registers rfc1867 as the reader, which consumes the
// body without buffering it — so the request has no body stream at all until the
// script asks for php://input, and the wrapper makes one on the spot. It is then
// empty and already marked fully read, so nothing is ever pulled through the
// SAPI for it: a request that owns its body by watching the reads would never
// hear about this one, and would leave a stream pair standing per request for
// the life of the worker.
//
// Same shape as hooks/test_post_body_streams_bounded, which covers the buffered
// (urlencoded) path: three sightings on one worker, the assertion on the delta
// between the second and the third, since the first also pays for whatever the
// request path allocates once.
//
// The counts are kept in $sharedState rather than in a temp file as that sibling
// keeps its own: the comparison only means anything within one worker lifetime,
// and worker-scoped state is exactly that. A file outlives both the worker and
// the container, so it has to be reset by hand at the first sighting to keep an
// interrupted run from leaving counts behind for the next one to compare against.

$t = new TestCase('multipart_input_streams_bounded', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$t->assertNotEmpty('the multipart body reached $_FILES', $_FILES);

// Empty for multipart in every SAPI — rfc1867 has already consumed the body —
// but asking for it is what makes the wrapper create one.
$before = count(get_resources('stream'));
$t->assertSame('php://input is empty on a multipart POST', file_get_contents('php://input'), '');
$created = count(get_resources('stream')) - $before;

// The mechanism this test is here for, stated rather than assumed: the request
// had no body until that read, and the wrapper made one — a temp stream over a
// memory stream, both of which outlive the handle file_get_contents() closed.
// Without this, a build that stopped creating a body here would keep the counts
// below flat and read green while proving nothing.
$t->meta('streams_created_by_the_wrapper', $created);
$t->assertGreaterThan('asking for php://input created the body', $created, 0);

$counts = $sighting > 1 ? (array) ($sharedState['multipart_stream_counts'] ?? []) : [];
$counts[] = count(get_resources('stream'));
$sharedState['multipart_stream_counts'] = $counts;

$t->meta('stream_counts', $counts);
$t->meta('sighting', $sighting);

if ($sighting < 3) {
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    unset($sharedState['multipart_stream_counts']);
    $t->assertCount('all three sightings landed on this worker', $counts, 3);
    $t->assertSame(
        'open stream count does not grow from one multipart POST to the next',
        $counts[2] ?? null,
        $counts[1] ?? null
    );
}

$t->done();
