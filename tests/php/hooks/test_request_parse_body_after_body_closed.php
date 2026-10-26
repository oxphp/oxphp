<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() after the script closed the request's body itself. The
// body is a registered stream resource, so get_resources('stream') hands it to
// the script and fclose() works on it. The second parse then buffers a new body
// in place of the closed one, and the end of the request must close that new
// body as it closes any other — not leave it standing because the body it knew
// about is gone.
//
// A php://input handle opened before the close still names the closed body. The
// call must leave it to the end of the request, which closes the handles on a
// body that is gone as it does on one that is not: closing it runs its filters,
// and the call is the script's own.
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile); the assertion
// compares the second and third, as hooks/test_request_parse_body_streams_bounded
// does.

$t = new TestCase('request_parse_body_after_body_closed', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$t->assertSame('$_POST came from the body', $_POST['probe'] ?? null, 'closed-body');

if ($sighting > 1) {
    $t->assertKeyExists(
        'the request before left its handle in worker-scope state',
        $sharedState,
        'request_parse_body_closed_early_handle'
    );
    $t->assertFalse(
        'the end of the request before closed the handle on its closed body',
        is_resource($sharedState['request_parse_body_closed_early_handle'] ?? null)
    );
    unset($sharedState['request_parse_body_closed_early_handle']);
}

// The body is a temp stream with no URI, and the newest one: resource ids only
// grow inside a worker, and the script has opened nothing yet.
$bodyId = null;
foreach (get_resources('stream') as $id => $res) {
    if (stream_get_meta_data($res)['stream_type'] === 'TEMP'
        && !isset(stream_get_meta_data($res)['uri'])
        && ($bodyId === null || $id > $bodyId)) {
        $bodyId = $id;
    }
}
$t->assertNotNull('the request body is reachable as a stream resource', $bodyId);

if ($bodyId !== null) {
    $body = get_resources('stream')[$bodyId];
    rewind($body);
    $t->assertSame('and it is this request\'s body', stream_get_contents($body), 'probe=closed-body');
}
// Never read once the body is closed: it reads through the freed stream.
$early = fopen('php://input', 'r');
if ($bodyId !== null) {
    fclose($body);
}
// The body the call buffers can otherwise land in the closed body's memory, and
// the end of the request would then close the handle above through the new
// body's address, whatever it does about the closed one. A temp stream is two
// streams, so two spacers.
$spacers = [fopen('php://memory', 'r+'), fopen('php://memory', 'r+')];

$before = count(get_resources('stream'));
$parsed = request_parse_body();
$created = count(get_resources('stream')) - $before;
array_map('fclose', $spacers);

$t->assertSame('the second parse finds the body already read', $parsed, [[], []]);
$t->assertTrue('the call left the handle on the closed body open', is_resource($early));
// The premise: the call made a body of its own for the end of the request to
// close; without it the counts below stay flat on any build.
$t->meta('streams_created_by_the_second_parse', $created);
$t->assertGreaterThan('and buffers a body of its own', $created, 0);

$counts = $sighting > 1 ? (array) ($sharedState['request_parse_body_closed_counts'] ?? []) : [];
$counts[] = count(get_resources('stream'));
$sharedState['request_parse_body_closed_counts'] = $counts;

$t->meta('stream_counts', $counts);

if ($sighting < 3) {
    $sharedState['request_parse_body_closed_early_handle'] = $early;
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    fclose($early);
    unset($sharedState['request_parse_body_closed_counts']);
    $t->assertCount('all three sightings landed on this worker', $counts, 3);
    $t->assertSame(
        'open stream count does not grow when the body was closed before the call',
        $counts[2] ?? null,
        $counts[1] ?? null
    );
}

$t->done();
