<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_state_survives_fatals', 'hooks');

// A fatal leaves its frames where they stood, and a worker has to give back what
// they were holding before it serves anything else. One of those frames is the
// included script's, and a script does not own its variables: entering it moves
// them out of the handler's frame and hands them over, leaving the handler with
// copies it has stopped owning. Undoing only that move gives the value two
// owners, both of which then give it up — one release too many, every fatal.
//
// What that costs is not visible in the request that fatals. It is the worker's
// shared state: a value the handler holds by reference loses a reference per
// fatal, and a few fatals in, the state is simply gone — every request after
// that starts from an empty array, with no error anywhere to say so. (Worse when
// another request is holding the same value at the time, which is the
// multiplexed case this profile is about: then it is freed under that request
// rather than merely lost.)
//
// Two phases, because the loss is only observable from a later request: while
// the phase that fatals is still running it holds a reference of its own, which
// keeps the value alive however far its count has been driven down.
$phase = $_GET['phase'] ?? 'drain';

$fetch = static function (string $file, string $query = ''): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooks/{$file}{$query} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return $body;
};

if ($phase === 'drain') {
    $t->assertContains(
        'the probe key is seeded on the worker',
        $fetch('fixture_shared_state_probe.php', '?seed=1'),
        'probe:kept'
    );

    // Two ways in, because the handing-back has a different distance to travel in
    // each. Straight to the script that fatals, the frame holding the variables
    // is the one immediately below it; through a file that only requires another,
    // there is a frame in between that never held them — a script with no
    // variables of its own is never given the symbol table — and the handing-back
    // has to carry on past it. Getting only the first one right leaves the second
    // losing a reference per fatal exactly as before.
    //
    // Five rounds each: three is what it takes to spend the value's last
    // reference on an unfixed build, and the rest is margin. Each fatal is
    // followed by a request that succeeds — that is also what keeps the worker's
    // consecutive-error count from reaching the point where the worker retires
    // and takes the shared state with it for a reason that has nothing to do with
    // what is being tested.
    foreach (['fixture_shared_state_probe.php', 'fixture_shared_state_wrapper.php'] as $entry) {
        for ($round = 1; $round <= 5; $round++) {
            $t->assertMatch(
                "{$entry} round {$round}: the fatal ended its own request",
                $fetch($entry, '?fatal=1'),
                '#^HTTP/1\.[01] 500#'
            );
            $t->assertContains(
                "{$entry} round {$round}: the worker went on serving",
                $fetch('fixture_shared_state_probe.php'),
                'probe:'
            );
        }
    }

    $t->done();
    return;
}

// The phase that matters, run as a request of its own so nothing from the drain
// is still holding the value up.
$t->assertSame(
    'the worker still has the shared state it had before the fatals',
    $sharedState['fatal_probe'] ?? 'gone',
    'kept'
);

$t->done();
