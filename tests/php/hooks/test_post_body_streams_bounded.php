<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The buffered request body is a registered stream resource, and in every other
// SAPI the destruction of the request's resource list closes it. A worker's
// request has no resource list of its own — the one it uses belongs to the
// worker and is destroyed once, at teardown — so unless the end of a request
// closes the body explicitly, every POST leaves one behind for the life of the
// worker. It is not a fixed cost either: a temp stream holds the whole body, so
// the growth is proportional to the traffic's body size.
//
// Listed three times on purpose. The first sighting compiles the file and
// warms whatever the request path allocates once; the assertion compares the
// second and third, which differ only by one more request having been served.
// This profile pins PHP_WORKERS=1, so all three land on the same worker and the
// counts are comparable.
//
// The sighting number comes from the suite line rather than from the ledger's
// own length, and sighting 1 starts the ledger from scratch. Containers outlive
// a run (`run_all.sh --no-build` reuses them), so a run interrupted after one or
// two sightings would otherwise leave entries behind for the next run to compare
// against — counts from two different worker lifetimes, which can as easily read
// green as red.

$t = new TestCase('post_body_streams_bounded', 'hooks');

$ledger = sys_get_temp_dir() . '/oxphp_post_body_streams_ledger.json';
$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

// Both routes to a body stream in one request: the SAPI reads the body to build
// $_POST, and php://input hands the same stream to the script.
$t->assertSame('$_POST came from the body', $_POST['probe'] ?? null, 'stream-count');
$t->assertNotEmpty('php://input returned the body', (string) file_get_contents('php://input'));

$seen = ($sighting > 1 && is_file($ledger))
    ? (array) json_decode((string) file_get_contents($ledger), true)
    : [];
$seen[] = count(get_resources('stream'));
file_put_contents($ledger, json_encode($seen));

$t->meta('stream_counts', $seen);
$t->meta('sighting', $sighting);

if ($sighting < 3) {
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    // Guarded rather than suppressed: TestCase turns every warning into an
    // ErrorException, and `@` does not stop a custom error handler from running.
    if (is_file($ledger)) {
        unlink($ledger);
    }
    // Guards the case where the ledger did not survive between sightings: two
    // counts have to be there for the comparison below to mean anything.
    $t->assertCount('all three sightings landed in the ledger', $seen, 3);
    $t->assertSame(
        'open stream count does not grow from one POST to the next',
        $seen[2] ?? null,
        $seen[1] ?? null
    );
}

$t->done();
