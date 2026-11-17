<?php

declare(strict_types=1);

// The comparator of a sort goes with its request when that request parks. The
// engine keeps the comparator of the sort in progress per thread and calls it
// from there for every comparison, so a request that resumes in the middle of
// its sort while another is parked in the middle of its own would go on
// sorting with the other request's comparator.
//
// A first request parks in its comparator for 300 ms, a second one sent
// meanwhile for 900 ms, so the first resumes while the second is still parked
// inside its sort.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$sharedState['cell_usort_log'] = [];
$sharedState['cell_usort_parked_first'] = false;
$sharedState['cell_usort_parked_second'] = false;

$firstSock = thread_cell_send('/tests/fibers/fixture_usort_parks.php?who=first&park=300');
$firstParked = thread_cell_wait_for($sharedState, 'cell_usort_parked_first');
$secondSock = thread_cell_send('/tests/fibers/fixture_usort_parks.php?who=second&park=900');
$secondParked = thread_cell_wait_for($sharedState, 'cell_usort_parked_second');
$first = json_decode(thread_cell_read($firstSock)['body'], true);
$second = json_decode(thread_cell_read($secondSock)['body'], true);
$log = $sharedState['cell_usort_log'];
$sharedState['cell_usort_running'] = null;

$t = new TestCase('usort_comparator_stays_with_its_request', 'fibers');
$t->meta('first', $first);
$t->meta('second', $second);
$t->meta('log', $log);

$t->assertTrue('the first request parked in its comparator', $firstParked);
$t->assertTrue('the second request parked in its comparator', $secondParked);
$t->assertSame(
    'the second request was still parked when the first resumed',
    $first['others_parked_at_resume']['cell_usort_parked_second'] ?? null,
    true
);

$foreign = array_values(array_filter($log, static fn (array $call): bool => $call['comparator'] !== $call['running']));
$t->assertSame('every comparison called the comparator of the request that was running', $foreign, []);
$t->assertNull('the first sort ended without an error', is_array($first) && array_key_exists('error', $first) ? $first['error'] : 'no answer');
$t->assertNull('the second sort ended without an error', is_array($second) && array_key_exists('error', $second) ? $second['error'] : 'no answer');
$t->assertSame('the first sort is sorted', $first['sorted'] ?? null, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
$t->assertSame('the second sort is sorted', $second['sorted'] ?? null, [1, 2, 3, 4, 5, 6, 7, 8, 9]);

$t->done();
