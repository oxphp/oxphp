<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A save handler can be open with no session active, and is then kept with its
// request the way an open session is.
//
// session_regenerate_id() closes the session, opens the handler again and asks
// it for a new id. When the handler gives none, the session is marked none and
// the handler is left open. Going by the session's status alone, that request
// would park leaving the handler's open state on the thread, and a request
// admitted meanwhile — which finds no session active, so may register a handler
// of its own — would drop the parked request's handler and open, write and close
// its own session over that state. The parked request's handler would then never
// be closed.
//
// A holder fails its regeneration this way and parks for a second. A neighbour
// sent in that second registers a handler of its own and opens its session.

$holderId = str_repeat('5e', 16);
$neighbourId = str_repeat('6f', 16);
$log = '/tmp/oxphp-session-regen-' . bin2hex(random_bytes(4)) . '.log';
$query = 'log=' . rawurlencode($log);

unset($sharedState['session_regen_parked']);
$holderSock = session_inner_send('/tests/fibers/fixture_session_regenerate_fails.php?role=holder&' . $query, $holderId, 8.0);
// Waiting parks this request, which lets the worker take the holder.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['session_regen_parked']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$neighbourSock = session_inner_send('/tests/fibers/fixture_session_regenerate_fails.php?role=neighbour&' . $query, $neighbourId, 8.0);

$holder = json_decode(session_inner_read($holderSock, 8.0), true);
$neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);

$entries = [];
foreach (file($log, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
    $entry = json_decode($line, true);
    if (is_array($entry)) {
        $entries[] = $entry;
    }
}
// The position of the first line at or after $from naming $tag and $call.
$at = static function (string $tag, string $call, int $from = 0) use ($entries): ?int {
    foreach ($entries as $i => $entry) {
        if ($i >= $from && $entry['tag'] === $tag && $entry['call'] === $call) {
            return $i;
        }
    }

    return null;
};

$t = new TestCase('session_handler_left_open_stays_with_its_request', 'fibers');
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);
$t->meta('log', $entries);

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the neighbour answered with JSON', is_array($neighbour));
if (!is_array($holder) || !is_array($neighbour)) {
    $t->done();
}

// The premise: the regeneration failed with the handler open and the session
// marked none.
$t->assertNotNull('the holder\'s regeneration failed', $holder['error'] ?? null);
$t->assertSame('leaving the session marked none', $holder['status'] ?? null, PHP_SESSION_NONE);
$reopened = $at('holder', 'open', (int) $at('holder', 'close') + 1);
$t->assertNotNull('after it had opened the handler again', $reopened);
$t->assertNotNull(
    'and asked it for an id',
    $reopened === null ? null : $at('holder', 'create_sid', $reopened)
);

// The handler went on to be closed through itself, by the holder's own request,
// and outlived that close.
$closed = $reopened === null ? null : $at('holder', 'close', $reopened);
$t->assertNotNull('the holder\'s handler was closed after it was opened again', $closed);
$destruct = $at('holder', 'destruct');
$t->assertTrue(
    'and was not destroyed before that',
    $closed !== null && ($destruct === null || $destruct > $closed)
);
$neighbourOpen = $at('neighbour', 'open');
$t->assertTrue(
    'before the neighbour opened its session',
    $closed !== null && $neighbourOpen !== null && $neighbourOpen > $closed
);

// And the neighbour ran once the holder was done.
$t->assertGreaterThan(
    'the neighbour was taken only once the holder was done',
    (float) ($neighbour['arrived_at'] ?? 0),
    (float) ($holder['done_at'] ?? INF)
);
$t->assertTrue('the neighbour was allowed to register its handler', $neighbour['configured'] ?? false);
$t->assertSame('and opened its own session', $neighbour['id'] ?? null, $neighbourId);

$t->done();
