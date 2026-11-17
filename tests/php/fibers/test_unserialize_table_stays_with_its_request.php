<?php

declare(strict_types=1);

// The table an open unserialize() call keeps goes with its request when that
// request parks. The engine keeps one table per thread for the unserialize()
// calls open on it and takes any call made while one is open for a nested
// one, so a call another request makes while this one is parked inside
// unserialize() — in an error handler the call raised — would push its values
// into this request's table, and its back-references would resolve to this
// request's objects.
//
// A holder parks inside unserialize() with an object of its own in the table.
// A neighbour sent meanwhile unserializes a pair that refers to its own object
// twice.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$sharedState['cell_unser_parked'] = false;
$holderSock = thread_cell_send('/tests/fibers/fixture_unserialize_parks.php?role=holder');
$parked = thread_cell_wait_for($sharedState, 'cell_unser_parked');
$neighbour = json_decode(thread_cell_read(thread_cell_send('/tests/fibers/fixture_unserialize_parks.php?role=neighbour'))['body'], true);
$holder = json_decode(thread_cell_read($holderSock)['body'], true);

$t = new TestCase('unserialize_table_stays_with_its_request', 'fibers');
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);

$t->assertTrue('the holder parked inside unserialize()', $parked);
$t->assertSame('the neighbour ran while the holder was parked', $neighbour['holder_parked'] ?? null, true);
$t->assertTrue("the neighbour's back-reference names its own object", $neighbour['same'] ?? null);
$t->assertSame("and not the holder's", $neighbour['second_who'] ?? null, 'neighbour');
$t->assertSame("the holder's call returned its own object", $holder['result_who'] ?? null, 'holder');
$t->assertTrue("and the holder's next call ran on a table of its own", ($holder['after']['same'] ?? false) && ($holder['after']['filled'] ?? false));

$t->done();
