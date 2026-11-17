<?php

declare(strict_types=1);

// A request that waits inside a comparison of array_multisort() keeps the
// worker thread for that wait, so no other request on the worker can sort in
// the meantime. The engine keeps the sort's table of comparison functions per
// thread for the whole call and puts the previous table back as the call ends:
// had the first request let the worker run a second sort while it waited, the
// first would put back what it found when it began — nothing — under the second
// sort, whose next comparison would call through it and crash the process.
//
// A first request waits 300 ms in its first comparison, a second one sent once
// the first has reached its wait waits 900 ms. While either waits, this request
// keeps trying to run, every 10 ms: it must not get a turn until the wait is
// over.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

// Closures rather than functions: the worker keeps a declared function for as
// long as it lives, and running this file a second time would declare it again.

// Waits, parked, until $state[$key] is true, noting each turn it gets.
$waitFor = static function (array &$state, string $key, array &$turns, float $seconds = 5.0): bool {
    $deadline = microtime(true) + $seconds;
    while (($state[$key] ?? false) !== true && microtime(true) < $deadline) {
        $turns[] = microtime(true);
        oxphp_usleep(10_000);
    }
    $turns[] = microtime(true);

    return ($state[$key] ?? false) === true;
};

// The turns this request got strictly inside a wait.
$turnsInside = static function (array $turns, ?array $wait): array {
    if ($wait === null) {
        return [];
    }

    return array_values(array_filter($turns, static fn (float $at): bool => $at > $wait[0] && $at < $wait[1]));
};

foreach (['first', 'second'] as $who) {
    unset($sharedState['multisort_entered_' . $who], $sharedState['multisort_wait_' . $who]);
}

$turns = [];
$fixture = '/tests/fibers/fixture_array_multisort_parks.php';
$firstSock = thread_cell_send("$fixture?who=first&park=300");
$firstEntered = $waitFor($sharedState, 'multisort_entered_first', $turns);
$secondSock = thread_cell_send("$fixture?who=second&park=900");
$secondEntered = $waitFor($sharedState, 'multisort_entered_second', $turns);
$first = json_decode(thread_cell_read($firstSock)['body'], true);
$second = json_decode(thread_cell_read($secondSock)['body'], true);
$firstWait = $sharedState['multisort_wait_first'] ?? null;
$secondWait = $sharedState['multisort_wait_second'] ?? null;

$t = new TestCase('array_multisort_keeps_its_comparisons_while_another_request_waits', 'fibers');
$t->meta('first', $first);
$t->meta('second', $second);
$t->meta('first_wait', $firstWait);
$t->meta('second_wait', $secondWait);
$t->meta('turns', count($turns));

$t->assertTrue('the first request reached its wait', $firstEntered);
$t->assertTrue('the second request reached its wait', $secondEntered);
$t->assertTrue(
    'this request was waiting for its turn before the first wait began',
    $firstWait !== null && $turns !== [] && $turns[0] < $firstWait[0]
);
$t->assertSame('no turn of this request fell inside the first wait', $turnsInside($turns, $firstWait), []);
$t->assertSame('no turn of this request fell inside the second wait', $turnsInside($turns, $secondWait), []);
$t->assertTrue('the first wait lasted its 300 ms', $firstWait !== null && $firstWait[1] - $firstWait[0] >= 0.29);
$t->assertTrue('the second wait lasted its 900 ms', $secondWait !== null && $secondWait[1] - $secondWait[0] >= 0.89);
$t->assertNull('the first sort ended without an error', is_array($first) && array_key_exists('error', $first) ? $first['error'] : 'no answer');
$t->assertNull('the second sort ended without an error', is_array($second) && array_key_exists('error', $second) ? $second['error'] : 'no answer');
$expected = ['strings' => ['a', 'b', 'c'], 'order' => [1, 2, 3]];
$t->assertSame('the first sort is sorted', $first['sorted'] ?? null, $expected);
$t->assertSame('the second sort is sorted', $second['sorted'] ?? null, $expected);

$t->done();
