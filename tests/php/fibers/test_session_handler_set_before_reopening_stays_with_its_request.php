<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A save handler a request registers between closing its session and opening
// it again stays that request's, even when the request parks with the reopened
// session open and a neighbour registered a handler of its own while the first
// one was parked.
//
// The handler a session is open with goes off the thread with it as the request
// parks, and the thread gets back what it had when the request resumed — the
// neighbour's handler — unless the request has configured another since, which
// then stays on the thread. When the session closes for good, the thread is
// handed back what it had while the session was parked, unless the request
// changed the handler in the meantime. A request that registered its second
// handler and reopened its session within one slice is told apart by the first
// of those two comparisons only: put the neighbour's handler back there, and
// the second comparison finds nothing changed, hands the thread the neighbour's
// handler for good, and the request's next session opens through it.
//
// A neighbour parks first; a holder then opens its session under a userland
// handler and parks for a second with it open. The neighbour wakes inside that
// second, registers a userland handler of its own and ends. The holder closes
// its session, registers a second handler, opens its session through it and
// parks with it open; then closes it, parks with it closed, and opens it a
// third time.

$t = new TestCase('session_handler_set_before_reopening_stays_with_its_request', 'fibers');

$holderId = str_repeat('a7', 16);
$neighbourId = str_repeat('b8', 16);
$log = '/tmp/oxphp-session-handler-relift-' . bin2hex(random_bytes(4)) . '.log';
$query = 'variant=relift&log=' . rawurlencode($log);

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

// How many lines name $tag, $call and $id.
$count = static function (string $tag, string $call, string $id) use ($entries): int {
    $n = 0;
    foreach ($entries as $line) {
        if ($line['tag'] === $tag && $line['call'] === $call && $line['id'] === $id) {
            $n++;
        }
    }

    return $n;
};

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the neighbour answered with JSON', is_array($neighbour));
if (!is_array($holder) || !is_array($neighbour)) {
    $t->done();
}

// The premises: the neighbour registered its handler while the holder's first
// session was parked open, and the holder registered its second one after
// closing that session.
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
$t->assertSame('the first session was written through the first handler', $count('holder', 'write', $holderId), 1);

// The reopened session, parked open, and the one after it both went through the
// handler the holder registered before reopening.
$t->assertSame('the holder reopened the same session', $holder['id_again'] ?? null, $holderId);
$t->assertSame('and opened it a third time', $holder['id_third'] ?? null, $holderId);
$t->assertSame(
    'both later sessions were read through the second handler',
    $count('holder-again', 'read', $holderId),
    2
);
$t->assertSame(
    'and written through it',
    $count('holder-again', 'write', $holderId),
    2
);
$t->assertSame(
    'the neighbour\'s handler never saw the holder\'s session',
    $count('neighbour', 'read', $holderId) + $count('neighbour', 'write', $holderId),
    0
);

$t->done();
