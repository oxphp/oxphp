<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A save handler a request registers after its parked session has closed stays
// that request's, even when a neighbour registered one of its own while the
// session was parked.
//
// The handler a session is open with goes off the thread with it, and the
// neighbour's goes back on as the request parks again — unless the request
// configured another one after closing its session, which then stays. Whether
// it did is told by comparing what the module names with the handler the
// session was open with. Registering a handler frees the previous one's
// callbacks before it builds its own, so the new callbacks can land at the
// addresses the old ones had: a comparison that holds no references of its own
// to what it compares against would take the new handler for the old one, put
// the neighbour's back, and open the request's next session through it.
//
// A neighbour parks first; a holder then opens its session under a userland
// handler and parks for a second with it open. The neighbour wakes inside that
// second, registers a userland handler of its own and ends. The holder closes
// its session, registers a second handler, parks briefly and opens its session
// again.

$t = new TestCase('session_handler_set_after_close_stays_with_its_request', 'fibers');

$holderId = str_repeat('be', 16);
$neighbourId = str_repeat('cf', 16);
$log = '/tmp/oxphp-session-handler-again-' . bin2hex(random_bytes(4)) . '.log';
$query = 'variant=again&log=' . rawurlencode($log);

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

$entries = [];
foreach (file($log, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
    $entry = json_decode($line, true);
    if (is_array($entry)) {
        $entry['data'] = base64_decode((string) ($entry['data'] ?? ''));
        $entries[] = $entry;
    }
}
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);
$t->meta('log', $entries);

// The position of the first line naming $tag and $call (and $id, if given).
$at = static function (string $tag, string $call, ?string $id = null) use ($entries): ?int {
    foreach ($entries as $i => $line) {
        if ($line['tag'] === $tag && $line['call'] === $call && ($id === null || $line['id'] === $id)) {
            return $i;
        }
    }

    return null;
};

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the neighbour answered with JSON', is_array($neighbour));
if (!is_array($holder) || !is_array($neighbour)) {
    $t->done();
}

// The premises: the neighbour registered its handler while the holder's session
// was parked open, and the holder registered its second one after closing it.
$t->assertTrue('the neighbour was allowed to register its own handler', $neighbour['configured'] ?? false);
$t->assertGreaterThan(
    'after the holder had opened its session',
    (float) ($neighbour['configured_at'] ?? 0),
    (float) ($holder['opened_at'] ?? INF)
);
$t->assertLessThan(
    'and before the holder was done with it',
    (float) ($neighbour['configured_at'] ?? INF),
    (float) ($holder['done_at'] ?? 0)
);
$t->assertGreaterThan(
    'the holder registered its second handler after closing its session',
    (float) ($holder['set_again_at'] ?? 0),
    (float) ($holder['closed_at'] ?? INF)
);
$t->assertNotNull('the holder\'s first session was written through its first handler', $at('holder', 'write', $holderId));

// Its second session went through the handler it registered for it.
$t->assertSame('the holder opened the same session again', $holder['id_again'] ?? null, $holderId);
$read = $at('holder-again', 'read', $holderId);
$write = $at('holder-again', 'write', $holderId);
$t->assertNotNull('the holder\'s second session was read through its second handler', $read);
$t->assertNotNull('and written through it', $write);
$destruct = $at('holder-again', 'destruct');
$t->assertTrue(
    'the second handler outlived the write',
    $write !== null && ($destruct === null || $destruct > $write)
);
$t->assertNull(
    'the neighbour\'s handler never saw the holder\'s session',
    $at('neighbour', 'read', $holderId) ?? $at('neighbour', 'write', $holderId)
);

$t->done();
