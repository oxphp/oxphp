<?php

declare(strict_types=1);

// Served by the worker that replaces the one a fatal error between two slices
// ended (see fixture_session_bailout_cycle.php): reports what that worker left
// of the session fixture_session_held_at_a_bailout.php held open, and of the one
// the destructor opened under ?orphan= — whether the store's lock on each is
// still taken and what its file holds — beside what the fixtures noted on the
// way.
//
// The lock is tried without waiting. The files handler locks with flock(), for
// which two descriptors of one file contend even inside one process, so a lock
// the old worker's descriptor still holds refuses this one instead of holding
// the thread. With ?start=1 the probe opens the session itself, as the next
// request with that cookie would — worth asking only once the lock is known to
// be free, since otherwise that wait would hold this worker for good.
//
// Driven by tests/scripts/verify_worker_session_closed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

set_error_handler(null);
set_exception_handler(null);
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');

$id = (string) ($_GET['id'] ?? '');

if (($_GET['start'] ?? '') === '1') {
    session_id($id);
    session_start();
    $marker = $_SESSION['marker'] ?? null;
    session_write_close();
    echo json_encode(['marker' => $marker]);

    return;
}

$noted = static function (string $path): mixed {
    return is_file($path) ? json_decode((string) file_get_contents($path), true) : null;
};

// "N;MODE;/path" names the directory last; empty means the temp directory.
$parts = explode(';', (string) session_save_path());
$dir = end($parts) ?: sys_get_temp_dir();

$inspect = static function (string $id) use ($dir): array {
    $file = $dir . '/sess_' . $id;
    $exists = $id !== '' && is_file($file);
    $free = null;
    if ($exists) {
        $fp = fopen($file, 'r');
        $free = flock($fp, LOCK_EX | LOCK_NB);
        if ($free) {
            flock($fp, LOCK_UN);
        }
        fclose($fp);
    }

    return [
        'file' => $file,
        'exists' => $exists,
        'free' => $free,
        'contents' => $exists ? (string) file_get_contents($file) : null,
    ];
};

echo json_encode([
    'request_count' => $requestCount,
    'held' => $noted('/tmp/oxphp-session-bailout-held'),
    'destructor' => $noted('/tmp/oxphp-session-bailout-destructor'),
    'session' => $inspect($id),
    'orphan' => $inspect((string) ($_GET['orphan'] ?? '')),
]);
