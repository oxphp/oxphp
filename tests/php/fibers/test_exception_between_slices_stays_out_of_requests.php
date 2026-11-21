<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// PHP the worker runs between two slices, in no request, can throw — here the
// destructor of a session save handler let go of once the session it was
// opened with has closed. That exception belongs to no request, so none of the
// requests the worker goes on to run may be handed it: neither one it resumes
// nor one it takes next. It is also dropped whole: its class has a destructor
// that throws in its turn, and a discard that clears it once leaves that one
// standing in its place.
//
// See fixture_session_handler_dtor_throws.php for the roles.

$run = static function (string $variant, string $holderId, string $neighbourId) use (&$sharedState): array {
    $marker = '/tmp/oxphp-session-dtor-throws-late-' . bin2hex(random_bytes(4));
    $query = 'variant=' . $variant . '&marker=' . rawurlencode($marker);

    unset($sharedState['session_dtor_throws_parked'], $sharedState['session_dtor_throws_late']);
    $neighbourSock = session_inner_send(
        '/tests/fibers/fixture_session_handler_dtor_throws.php?role=neighbour&' . $query,
        $neighbourId,
        8.0
    );
    $deadline = microtime(true) + 3.0;
    while (!isset($sharedState['session_dtor_throws_parked']) && microtime(true) < $deadline) {
        oxphp_usleep(10_000);
    }

    $holderSock = session_inner_send(
        '/tests/fibers/fixture_session_handler_dtor_throws.php?role=holder&' . $query,
        $holderId,
        8.0
    );
    // Parked on a timer rather than on either socket, so that when the holder
    // ends nothing of this request is ready to be resumed: what the worker does
    // next is the one the variant sets up.
    oxphp_sleep(2.5);

    $holder = json_decode(session_inner_read($holderSock, 8.0), true);
    $neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);
    $late = is_file($marker) ? (string) file_get_contents($marker) : '';
    if (isset($sharedState['session_dtor_throws_late'])) {
        fclose($sharedState['session_dtor_throws_late']);
        unset($sharedState['session_dtor_throws_late']);
    }
    if (is_file($marker)) {
        unlink($marker);
    }

    return ['holder' => $holder, 'neighbour' => $neighbour, 'late' => $late];
};

$t = new TestCase('exception_between_slices_stays_out_of_requests', 'fibers');

$resume = $run('resume', str_repeat('b1', 16), str_repeat('c2', 16));
$t->meta('resume', $resume);
$t->assertSame('[resume] the holder answered', $resume['holder']['who'] ?? null, 'holder');
$t->assertTrue('[resume] the neighbour registered its own handler while the holder was parked', $resume['neighbour']['configured'] ?? false);
$t->assertSame('[resume] the neighbour resumed into its own session and answered', $resume['neighbour']['who'] ?? null, 'neighbour');

$admission = $run('admission', str_repeat('d3', 16), str_repeat('e4', 16));
$t->meta('admission', $admission);
$t->assertSame('[admission] the holder answered', $admission['holder']['who'] ?? null, 'holder');
$t->assertTrue('[admission] the holder sent the late request', $admission['holder']['sent_late'] ?? false);
$t->assertTrue('[admission] the neighbour registered its own handler while the holder was parked', $admission['neighbour']['configured'] ?? false);
$t->assertSame('[admission] the request taken after the holder ran its handler', $admission['late'], 'ran');

$t->done();
