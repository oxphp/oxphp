<?php

declare(strict_types=1);

// A session held open beside a connection the worker shares, for the tests that
// a request with its session open can still wait for that connection, and that
// a wait the two cannot both get out of is given up at once.
//
// ?role=hold takes the connection with a query that runs for a second. A claim
// on a shared connection lasts until the request that took it ends.
//
// ?role=session opens its client's session and, with it open, runs a short
// query on the connection.
//
// ?role=claim_then_session takes the connection with a short query, parks for
// ?park= seconds, then opens its client's session.
//
// The connection is $sharedState[?key=], created by the test before any of
// these run. Each role answers with one JSON object of what it did and when.
//
// Not a TestCase — the body is read by the request that started this one.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default; the id is the client's
// cookie and the test reads it back.
ini_set('session.use_strict_mode', '0');

$role = (string) ($_GET['role'] ?? '');
$pdo = $sharedState[(string) ($_GET['key'] ?? '')] ?? null;
$out = ['role' => $role];

header('Content-Type: application/json');

if (!$pdo instanceof PDO) {
    echo json_encode($out + ['error' => 'the test did not create the connection']);
    return;
}

try {
    switch ($role) {
        case 'hold':
            $sharedState['session_claim_holding'] = true;
            $out['query_started_at'] = microtime(true);
            $out['result'] = $pdo->query('SELECT SLEEP(1)')->fetchColumn();
            $out['query_done_at'] = microtime(true);
            break;

        case 'session':
            session_start();
            $_SESSION['who'] = 'session';
            $out['open_when_querying'] = session_status() === PHP_SESSION_ACTIVE;
            $out['query_started_at'] = microtime(true);
            try {
                $out['result'] = $pdo->query('SELECT 7')->fetchColumn();
            } catch (\Throwable $e) {
                $out['error'] = get_class($e) . ': ' . $e->getMessage();
            }
            $out['query_done_at'] = microtime(true);
            $out['id'] = session_id();
            session_write_close();
            $out['closed_at'] = microtime(true);
            break;

        case 'claim_then_session':
            $out['result'] = $pdo->query('SELECT 3')->fetchColumn();
            $sharedState['session_claim_holding'] = true;
            oxphp_sleep((float) ($_GET['park'] ?? 0.3));
            $out['wait_started_at'] = microtime(true);
            session_start();
            $out['started_at'] = microtime(true);
            $out['id'] = session_id();
            $out['who'] = $_SESSION['who'] ?? 'none';
            session_write_close();
            break;

        default:
            $out['error'] = "unknown role: $role";
    }
} catch (\Throwable $e) {
    $out['error'] = get_class($e) . ': ' . $e->getMessage();
}

echo json_encode($out);
