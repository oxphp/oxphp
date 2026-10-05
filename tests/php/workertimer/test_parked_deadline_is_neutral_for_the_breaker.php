<?php

declare(strict_types=1);

require_once __DIR__ . '/../breaker/breaker_probe.php';

// A limit that runs out while its request is parked is a deadline like any
// other, and neutral for the worker's consecutive-error breaker.
//
// ?action=park sleeps for three seconds without setting a limit, under the
// runtime hooks, so the request is parked when its two seconds are over and is
// ended by them when it is resumed. Three of those in a row are three
// deadlines, and a deadline is the server ending a request, not the handler
// failing at it: the worker must still be serving afterwards. Counted as
// failures instead, the third would retire it.
//
// ?action=start records where the worker's request count and the recycle
// counters stood, and ?action=check compares with that. Kept in a file rather
// than in the worker's memory, because a retire would take the worker's memory
// with it and the check is the request that has to report the retire.

$stateFile = '/tmp/oxphp-workertimer-breaker.json';

$action = $_GET['action'] ?? '';

if ($action === 'park') {
    // Hooked in this profile: parks this request for the whole three seconds.
    usleep(3_000_000);
    echo "unreachable: max_execution_time must end this request\n";
    return;
}

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('parked_deadline_is_neutral_for_the_breaker', 'workertimer');

$count = OxPHP\Server\Worker::current()->requestCount();
$recycles = breaker_recycles();
$t->assertNotNull('/metrics exposes the worker-mode block', $recycles);

if ($action === 'start') {
    if ($recycles !== null) {
        file_put_contents($stateFile, json_encode([
            'count' => $count,
            'recycles' => $recycles,
        ]));
    }
    $t->done();
}

$start = json_decode((string) @file_get_contents($stateFile), true);
$t->assertTrue('the start line recorded where things stood', is_array($start));

if (is_array($start) && $recycles !== null) {
    // The start request, three parked ones, and this one.
    $t->assertSame('still the same worker', $count, $start['count'] + 4);
    $t->assertSame(
        'none of the three tripped the breaker',
        $recycles['error'],
        $start['recycles']['error']
    );
    $t->assertSame('no worker was recycled at all', $recycles['total'], $start['recycles']['total']);
}

$t->done();
