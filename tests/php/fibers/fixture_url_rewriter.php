<?php

declare(strict_types=1);

// Requests that put something into PHP's URL rewriter — the output handler that
// appends variables to the links and forms a response prints — for the tests
// that what one request adds reaches its own response and no other.
//
// ?role=trans starts a session the deprecated way, with the id carried in URLs:
// session.use_only_cookies off, session.use_trans_sid on, under a session name
// no client sends a cookie for, so every request gets a new id and the module
// adds it to the rewriter. It closes the session, prints a link, and with
// &park=<s> parks that long before printing the id it got and the SID constant
// it reads. The link is rewritten only when the response is flushed at the end,
// so a parked request's link is rewritten after whatever ran in the window.
// With &hold=1 the session stays open while the request parks, which keeps the
// worker from taking any other request until this one has closed it and ended.
//
// ?role=var adds ?tok=<value> to the rewriter with output_add_rewrite_var(),
// with url_rewriter.tags set so links are rewritten, prints a link, and with
// &park=<s> parks that long.
//
// ?role=sid opens nothing and reports the SID constant.
//
// A request that parks sets $sharedState['url_rewriter_parked'] first.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$park = (float) ($_GET['park'] ?? 0);

// Reading SID is deprecated from PHP 8.4; silenced for the same reason.
$sid = static fn (): ?string => defined('SID') ? (string) @constant('SID') : null;

if ($role === 'trans') {
    // Both deprecated from PHP 8.4; silenced so the notice is not part of the
    // body the test reads.
    @ini_set('session.use_only_cookies', '0');
    @ini_set('session.use_trans_sid', '1');
    ini_set('session.use_strict_mode', '0');
    ini_set('session.name', 'OXRWSID');
    $hold = isset($_GET['hold']);
    session_start();
    $id = session_id();
    $sidAtStart = $sid();
    if (!$hold) {
        session_write_close();
    }

    echo '<a href="/next.php">next</a>' . "\n";
    if ($park > 0) {
        $sharedState['url_rewriter_parked'] = microtime(true);
        oxphp_sleep($park);
    }
    $held = session_status() === PHP_SESSION_ACTIVE;
    session_write_close();
    echo '<p>' . json_encode([
        'id' => $id,
        'sid_at_start' => $sidAtStart,
        'sid' => $sid(),
        'held' => $held,
        'ended_at' => microtime(true),
    ]) . '</p>' . "\n";
    return;
}

if ($role === 'var') {
    // Links too: by default the rewriter adds a script's variables to forms only.
    ini_set('url_rewriter.tags', 'a=href');
    output_add_rewrite_var('tok', (string) ($_GET['tok'] ?? ''));
    echo '<a href="/next.php">next</a>' . "\n";
    if ($park > 0) {
        $sharedState['url_rewriter_parked'] = microtime(true);
        oxphp_sleep($park);
    }
    return;
}

if ($role === 'sid') {
    header('Content-Type: application/json');
    echo json_encode(['sid' => $sid(), 'at' => microtime(true)]);
    return;
}

http_response_code(400);
echo "unknown role: $role";
