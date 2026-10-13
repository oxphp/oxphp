<?php

declare(strict_types=1);

// Parks before any session is open on the worker, in one of three ways, and
// reports what it finds of a session when it wakes, when it woke, and when its
// own session_start() returned.
//
// ?kind= picks the wait, one for each place the worker resumes a parked request
// from: a timer (oxphp_sleep()), a descriptor (a hooked stream_select() on a
// socket an async task connects to) and a promise (oxphp_async_await() on a
// task). Each comes due 0.4s after the park. The test opens another client's
// session on the worker inside those 0.4s and keeps it open past them.

set_error_handler(null);
set_exception_handler(null);

$kind = (string) ($_GET['kind'] ?? '');
$delay = 0.4;

switch ($kind) {
    case 'sleep':
        $parkedAt = microtime(true);
        $sharedState['session_parked_before_it'][$kind] = $parkedAt;
        oxphp_sleep($delay);
        $resumedAt = microtime(true);
        // When the timer was due, which is what the window is measured against.
        $dueAt = $parkedAt + $delay;
        break;

    case 'io':
        $server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
        $addr = stream_socket_get_name($server, false);
        $connector = oxphp_async(static function (string $addr, float $delay): float {
            oxphp_sleep($delay);
            $connectedAt = microtime(true);
            $client = stream_socket_client("tcp://$addr", $errno, $errstr, 5.0);
            if ($client !== false) {
                fwrite($client, 'x');
                fclose($client);
            }

            return $connectedAt;
        }, $addr, $delay);
        $parkedAt = microtime(true);
        $sharedState['session_parked_before_it'][$kind] = $parkedAt;
        $read = [$server];
        $write = null;
        $except = null;
        stream_select($read, $write, $except, 5);
        $resumedAt = microtime(true);
        break;

    case 'await':
        $task = oxphp_async(static function (float $delay): float {
            oxphp_sleep($delay);

            return microtime(true);
        }, $delay);
        $parkedAt = microtime(true);
        $sharedState['session_parked_before_it'][$kind] = $parkedAt;
        $dueAt = oxphp_async_await($task, 5.0);
        $resumedAt = microtime(true);
        break;

    default:
        http_response_code(400);
        echo "unknown kind: $kind";
        return;
}

// Read before anything below can start a session: once one has started, a clean
// thread and one with another client's session left on it look the same from
// inside.
$pre = [
    'session_isset' => isset($_SESSION),
    'session_id' => session_id(),
    'who' => $_SESSION['who'] ?? 'none',
];

if ($kind === 'io') {
    $conn = stream_socket_accept($server, 1.0);
    if ($conn !== false) {
        fclose($conn);
    }
    fclose($server);
    $dueAt = oxphp_async_await($connector, 5.0);
}

// The probe's signature, as in fixture_session_probe.php: session_start() on a
// session already active answers TRUE with a notice and never reads the cookie.
$notice = null;
set_error_handler(static function (int $errno, string $message) use (&$notice): bool {
    $notice = $message;

    return true;
});
// PHP 8.6 turns session.use_strict_mode on by default; the id is the client's
// cookie and the test reads it back.
ini_set('session.use_strict_mode', '0');
session_start();
$startedAt = microtime(true);
restore_error_handler();
$id = session_id();
$who = $_SESSION['who'] ?? 'none';
session_write_close();

header('Content-Type: application/json');
echo json_encode([
    'kind' => $kind,
    'parked_at' => $parkedAt,
    'due_at' => $dueAt,
    'resumed_at' => $resumedAt,
    'started_at' => $startedAt,
    'pre' => $pre,
    'notice' => $notice,
    'id' => $id,
    'who' => $who,
]);
