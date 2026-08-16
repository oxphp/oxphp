<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The buffered request body is a registered stream resource like any other, so
// get_resources('stream') hands the script a full handle to it — with a
// reference of its own, which is what makes fclose() on it work. Worker mode
// then closes the same body a second time at the end of the request, because the
// resource list that would have done it belongs to the worker rather than to the
// request. One fclose() on its own shows nothing: the second free lands in
// _php_stream_free()'s in_free recursion guard, which reads memory that is freed
// but not yet reused and returns quietly. It becomes visible as soon as the block
// IS reused — the end of the request then closes whatever now lives at that
// address, which is a stream the script opened and is still holding.
//
// Hence the shape: close the body, immediately open a batch of php://temp streams
// so that one of them lands on the freed block, park them in worker-scope state,
// and ask the next request what became of them. On a build that closes the body
// by address, exactly one of the batch comes back destroyed.
//
// Destroyed, not merely closed: a handle nothing else references carries the only
// reference to its zend_resource, so closing it without keeping the resource —
// which is what an unqualified close does — frees that 24-byte block too. The
// parked array is then left naming freed memory, and every way of asking a handle
// how it is doing reads it. So the question goes to the resource list instead:
// resource ids are never recycled inside a worker, so an id that is still listed
// is still the same resource, and one that is gone was destroyed. Nothing in the
// reporting path touches a handle until the list has said they all survived.
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile), adjacent in the
// suite so that nothing else opens or closes a stream in between:
//   1. GET  — record which stream resources the worker already holds. No body of
//             its own, so none of them is one.
//   2. POST — everything that is not in that baseline is this request's body and
//             the memory stream it encloses. Close them, open the batch, park it
//             along with the ids of every resource it registered.
//   3. GET  — every one of those ids must still be in the resource list, and
//             every parked handle must still read back what was written into it.
//
// $sharedState is the per-worker store the worker fixture keeps; PHP `include`
// runs in the includer's scope, so this file reaches it.

$t = new TestCase('body_stream_not_double_closed', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

// A local rather than a const: this file is included once per request for the
// life of the worker, and a top-level const would be redefined on the second.
$probeCount = 12;
$probeBody = static fn (int $i): string => 'ox-body-probe-' . $i;

if ($sighting === 1) {
    $sharedState['body_double_close_baseline'] = array_keys(get_resources('stream'));
    $t->assertTrue(
        'baseline of the streams this worker already holds is recorded',
        is_array($sharedState['body_double_close_baseline'])
    );
    $t->meta('baseline_count', count($sharedState['body_double_close_baseline']));
} elseif ($sighting === 2) {
    $t->assertKeyExists(
        'the baseline sighting ran on this worker first',
        $sharedState,
        'body_double_close_baseline'
    );
    $baseline = (array) ($sharedState['body_double_close_baseline'] ?? []);

    // Read before anything is closed: $_POST is built from the body during the
    // request's input rebuild, so this says the body was really there.
    $t->assertSame('$_POST came from this request\'s body', $_POST['probe'] ?? null, 'double-close');

    $fresh = [];
    foreach (get_resources('stream') as $id => $res) {
        if (!in_array($id, $baseline, true)) {
            $fresh[$id] = $res;
        }
    }
    $t->assertGreaterThan(
        'the request body is reachable as a stream resource',
        count($fresh),
        0
    );

    $types = [];
    foreach ($fresh as $res) {
        $meta = stream_get_meta_data($res);
        $types[] = (string) ($meta['stream_type'] ?? '?');
    }
    $t->meta('body_stream_types', $types);
    $t->assertTrue(
        'and it is the buffered body pair (a temp stream over a memory stream)',
        in_array('TEMP', $types, true) && in_array('MEMORY', $types, true)
    );

    // Closing the temp stream closes the memory stream it encloses, so the
    // second handle is already retired by the time the loop reaches it. Asked
    // rather than assumed: fclose() on a retired resource is a TypeError.
    foreach ($fresh as $res) {
        if (is_resource($res)) {
            fclose($res);
        }
    }

    // The batch that has to survive. Opened right after the close so that the
    // freed blocks are still at the head of the allocator's free lists.
    $liveBefore = array_keys(get_resources('stream'));
    $handles = [];
    for ($i = 0; $i < $probeCount; $i++) {
        $handle = fopen('php://temp', 'r+');
        fwrite($handle, $probeBody($i));
        $handles[] = $handle;
    }
    // Every resource the batch registered, which is two per handle — php://temp
    // is a temp stream over a memory stream, and either of them can be the one
    // that takes the freed block.
    $ids = array_values(array_diff(array_keys(get_resources('stream')), $liveBefore));

    $sharedState['body_double_close_probes'] = $handles;
    $sharedState['body_double_close_probe_ids'] = $ids;

    $t->assertCount('the batch is open and parked for the next request', $handles, $probeCount);
    $t->meta('probe_resource_ids', count($ids));
    $t->assertGreaterThan('and every handle in it registered a resource', count($ids), $probeCount - 1);
} else {
    $t->assertKeyExists(
        'the request before this one parked its batch',
        $sharedState,
        'body_double_close_probe_ids'
    );
    $ids = (array) ($sharedState['body_double_close_probe_ids'] ?? []);
    $t->assertGreaterThan('with the ids of everything it registered', count($ids), $probeCount - 1);

    // The whole question, and asked of the resource list rather than of the
    // handles: reading a handle whose resource the previous request freed is
    // itself a use-after-free, and a test must not perform one to show that it is
    // possible. Reading the array is safe — copying it copies no element.
    $live = get_resources('stream');
    $retired = [];
    foreach ($ids as $id) {
        if (!isset($live[$id])) {
            $retired[] = $id;
        }
    }
    $t->meta('retired_resource_ids', $retired);
    $t->assertCount(
        'no stream the previous request parked was closed by the end of that request',
        $retired,
        0
    );

    if ($retired === []) {
        // Only now, with the list saying every handle is still there.
        $handles = (array) ($sharedState['body_double_close_probes'] ?? []);
        $t->assertCount('all of it arrived', $handles, $probeCount);

        $damaged = [];
        foreach ($handles as $i => $handle) {
            try {
                rewind($handle);
                $got = stream_get_contents($handle);
                if ($got !== $probeBody($i)) {
                    $damaged[$i] = var_export($got, true);
                }
            } catch (\Throwable $e) {
                $damaged[$i] = get_class($e) . ': ' . $e->getMessage();
            }
        }
        $t->meta('damaged_handles', $damaged);
        $t->assertCount('and every one of them still reads back what it was given', $damaged, 0);

        foreach ($handles as $handle) {
            fclose($handle);
        }
        unset($sharedState['body_double_close_probes']);
    }
    // The parked handles are left alone when something is retired: one element of
    // that array names a freed resource, and taking the array apart reads it just
    // as surely as using it would. The suite visits these three sightings once,
    // so nothing after this touches it, and the run stays red for its own reason
    // instead of ending in the allocator.

    unset($sharedState['body_double_close_probe_ids'], $sharedState['body_double_close_baseline']);
}

$t->done();
