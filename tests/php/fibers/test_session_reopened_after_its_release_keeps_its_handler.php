<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A session the end of a request opens again, after that request's own session
// has been given back, is written and closed through the handler it was opened
// with — not through the one the thread is handed back once the request is
// over.
//
// The end of a request writes its session and then, on PHP 8.5, lets go of the
// backtrace of its last fatal, whose arguments can have destructors of their
// own. A destructor there that calls session_start() opens a session through
// whatever handler the thread has at that moment, which for a request whose
// session was parked open is still the one it was opened with. Given back after
// the thread is handed its other handler, that session would be written through
// the neighbour's handler, and the files handler's lock on it lost for the life
// of the process: every later request with that cookie would wait for it.
//
// A neighbour parks first; a holder then opens its session under the files
// handler and parks for a second with it open. The neighbour wakes inside that
// second, registers a userland handler of its own and ends. The holder closes
// its session and raises a fatal its error handler takes over, from a call
// given an object whose destructor opens the session again and leaves it open.
//
// Before 8.5 there is no backtrace and the destructor runs as the call returns,
// in the middle of the request; the outcome checked below holds there as well,
// and the premise that the destructor ran after the release is checked on 8.5
// only.

$t = new TestCase('session_reopened_after_its_release_keeps_its_handler', 'fibers');

// Random rather than fixed: on a build where the lock is lost, a second run
// against the same server would wait for it.
$holderId = bin2hex(random_bytes(16));
$neighbourId = bin2hex(random_bytes(16));
$log = '/tmp/oxphp-session-reopen-' . bin2hex(random_bytes(4)) . '.log';
$query = 'log=' . rawurlencode($log);

unset($sharedState['session_reopen_parked']);
$neighbourSock = session_inner_send(
    '/tests/fibers/fixture_session_reopened_after_release.php?role=neighbour&' . $query,
    $neighbourId,
    8.0
);
// Waiting parks this request, which lets the worker take the neighbour.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['session_reopen_parked']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}

// Reading the holder's response parks this request until the holder is over;
// what its end left is given back before this request resumes.
$holder = json_decode(session_inner_request(
    '/tests/fibers/fixture_session_reopened_after_release.php?role=holder&' . $query,
    $holderId,
    8.0
), true);
$neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);

$entries = [];
foreach (file($log, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
    $entry = json_decode($line, true);
    if (is_array($entry)) {
        $entries[] = $entry;
    }
}
$destructor = null;
$neighbourSaw = 0;
foreach ($entries as $entry) {
    if ($entry['tag'] === 'destructor') {
        $destructor = $entry;
    } elseif ($entry['tag'] === 'neighbour' && ($entry['id'] ?? '') === $holderId) {
        $neighbourSaw++;
    }
}

// The store, as the files handler keeps it: "N;MODE;/path" names the
// directory last, and empty means the temp directory. The lock is tried
// without waiting — flock() locks of two descriptors of one file contend even
// inside one process, so one still held refuses this one.
$parts = explode(';', (string) ($holder['save_path'] ?? ''));
$file = (end($parts) ?: sys_get_temp_dir()) . '/sess_' . $holderId;
$exists = is_file($file);
$free = null;
if ($exists) {
    $fp = fopen($file, 'r');
    $free = flock($fp, LOCK_EX | LOCK_NB);
    if ($free) {
        flock($fp, LOCK_UN);
    }
    fclose($fp);
}
$contents = $exists ? (string) file_get_contents($file) : '';

$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);
$t->meta('log', $entries);
$t->meta('file', ['path' => $file, 'exists' => $exists, 'free' => $free, 'contents' => $contents]);

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the neighbour answered with JSON', is_array($neighbour));
if (!is_array($holder) || !is_array($neighbour)) {
    $t->done();
}

// The premises: the neighbour registered its handler while the holder's session
// was parked open, and the destructor opened the holder's session again
// through the files handler.
$t->assertSame('the holder opened the session under its cookie', $holder['id'] ?? null, $holderId);
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
$t->assertNotNull('the destructor ran', $destructor);
$t->assertTrue('and opened the session again', $destructor['started'] ?? false);
$t->assertSame('under the holder\'s id', $destructor['id'] ?? null, $holderId);
$t->assertSame('through the files handler', $destructor['module'] ?? null, 'files');
if (PHP_VERSION_ID >= 80500) {
    $t->assertSame(
        'after the end of the request had given the holder\'s session back',
        $destructor['before']['id'] ?? null,
        ''
    );
}

$t->assertSame('the neighbour\'s handler never saw the holder\'s session', $neighbourSaw, 0);
$t->assertTrue('the session the destructor opened was written to its store', str_contains($contents, 'reopened|'));
$t->assertTrue('and the store\'s lock on it was let go', $free === true);

$t->done();
