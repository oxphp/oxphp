<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A session parked open is written and closed through the handler it was opened
// with, whatever the requests that run in the meantime configure.
//
// The open session is taken off the thread while its request is parked, and a
// request that runs in that window finds no session active — so nothing stops
// it from registering a save handler of its own, switching the save handler or
// the serializer. Those are the session module's settings, one set per thread,
// and the parked session's write and close go through whatever the module names
// when they run. So the handler and serializer a session is open with go off the
// thread with it, and come back with it.
//
// A neighbour parks first; a holder then opens its session under a userland
// handler with the php_serialize serializer and parks for a second with it
// open. The neighbour wakes inside that second and reconfigures the module for
// itself, then opens its own session, which waits for the holder's to close.
// Twice: once registering a userland handler of its own with the php_binary
// serializer, once switching to the files handler — which, with a userland
// handler's open state underneath it, would be handed state that is not its
// own.

// Closures rather than functions: a function a test file declares is declared on
// the worker, which outlives the request.
$run = static function (string $variant, string $holderId, string $neighbourId) use (&$sharedState): array {
    $log = '/tmp/oxphp-session-handler-pin-' . $variant . '-' . bin2hex(random_bytes(4)) . '.log';
    $query = 'variant=' . $variant . '&log=' . rawurlencode($log);

    unset($sharedState['session_handler_pin_parked']);
    $neighbourSock = session_inner_send(
        '/tests/fibers/fixture_session_handler_pin.php?role=neighbour&' . $query,
        $neighbourId,
        8.0
    );
    // Waiting parks this request, which lets the worker take the neighbour.
    $deadline = microtime(true) + 3.0;
    while (!isset($sharedState['session_handler_pin_parked']) && microtime(true) < $deadline) {
        oxphp_usleep(10_000);
    }

    $holder = json_decode(session_inner_request(
        '/tests/fibers/fixture_session_handler_pin.php?role=holder&' . $query,
        $holderId,
        8.0
    ), true);
    $neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);

    $lines = [];
    foreach (file($log, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
        $entry = json_decode($line, true);
        if (is_array($entry)) {
            $entry['data'] = base64_decode((string) ($entry['data'] ?? ''));
            $lines[] = $entry;
        }
    }

    return ['holder' => $holder, 'neighbour' => $neighbour, 'log' => $lines];
};

// The position of the first line naming $tag and $call (and $id, if given).
$at = static function (array $log, string $tag, string $call, ?string $id = null): ?int {
    foreach ($log as $i => $line) {
        if ($line['tag'] === $tag && $line['call'] === $call && ($id === null || $line['id'] === $id)) {
            return $i;
        }
    }

    return null;
};

$t = new TestCase('session_handler_stays_with_its_open_session', 'fibers');

$runs = [
    'user' => $run('user', str_repeat('7a', 16), str_repeat('8b', 16)),
    'files' => $run('files', str_repeat('9c', 16), str_repeat('ad', 16)),
];

foreach ($runs as $variant => $run) {
    $holder = $run['holder'];
    $neighbour = $run['neighbour'];
    $log = $run['log'];
    $t->meta($variant, $run);

    $t->assertTrue("[$variant] the holder answered with JSON", is_array($holder));
    $t->assertTrue("[$variant] the neighbour answered with JSON", is_array($neighbour));
    if (!is_array($holder) || !is_array($neighbour)) {
        continue;
    }
    $holderId = (string) ($holder['id'] ?? '');

    // The premise: the neighbour reconfigured the module while the holder's
    // session was open, and was allowed to.
    $t->assertTrue("[$variant] the neighbour was allowed to configure its own handler", $neighbour['configured'] ?? false);
    $t->assertGreaterThan(
        "[$variant] after the holder had opened its session",
        (float) ($neighbour['configured_at'] ?? 0),
        (float) ($holder['opened_at'] ?? INF)
    );
    $t->assertLessThan(
        "[$variant] and before the holder was done with it",
        (float) ($neighbour['configured_at'] ?? INF),
        (float) ($holder['done_at'] ?? 0)
    );

    // The holder's session went through its own handler, in its own format.
    $write = $at($log, 'holder', 'write', $holderId);
    $t->assertNotNull("[$variant] the holder's session was written through its own handler", $write);
    $t->assertSame(
        "[$variant] with its own serializer",
        $write === null ? null : $log[$write]['data'],
        serialize(['who' => 'holder'])
    );
    $t->assertNotNull(
        "[$variant] and closed through it",
        $at(array_slice($log, (int) $write), 'holder', 'close')
    );
    $destruct = $at($log, 'holder', 'destruct');
    $t->assertTrue(
        "[$variant] its handler outlived the write",
        $write !== null && ($destruct === null || $destruct > $write)
    );
    $t->assertNull(
        "[$variant] the neighbour's handler never saw the holder's session",
        $at($log, 'neighbour', 'write', $holderId)
            ?? $at($log, 'neighbour', 'read', $holderId)
    );

    // And the neighbour's went through what it configured, once the holder's
    // had closed.
    $t->assertGreaterThan(
        "[$variant] the neighbour's session_start() returned only once the holder had closed its session",
        (float) ($neighbour['started_at'] ?? 0),
        (float) ($holder['closed_at'] ?? INF)
    );
    $neighbourId = (string) ($neighbour['id'] ?? '');
    $t->assertNull(
        "[$variant] the holder's handler never saw the neighbour's session",
        $at($log, 'holder', 'read', $neighbourId)
            ?? $at($log, 'holder', 'write', $neighbourId)
    );
    if ($variant === 'user') {
        $neighbourWrite = $at($log, 'neighbour', 'write', $neighbourId);
        $t->assertNotNull("[$variant] the neighbour's session was written through its own handler", $neighbourWrite);
        $t->assertSame(
            "[$variant] with its own serializer",
            $neighbourWrite === null ? null : $log[$neighbourWrite]['data'],
            chr(strlen('who')) . 'who' . serialize('neighbour')
        );
    } else {
        $t->assertSame("[$variant] the neighbour's session was opened with the files handler", $neighbour['module'] ?? null, 'files');
    }
}

$t->done();
