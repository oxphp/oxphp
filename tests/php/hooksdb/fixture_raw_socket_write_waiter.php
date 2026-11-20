<?php

declare(strict_types=1);

// Writes a command on the shared raw connection that fixture_raw_socket_write_hold
// is parked on, so the write has to wait for that holder, and lets the request
// that fired it close the connection in the meantime.
//
// Its log goes to a file of its own, named by the caller's key, so the caller can
// read what ended this request.
$key = preg_replace('/[^a-z0-9]/', '', (string) ($_GET['key'] ?? ''));
ini_set('log_errors', '1');
ini_set('error_log', "/tmp/oxphp-rawwrite-waiter-{$key}.log");

try {
    $sock = $sharedState["rawwrite_sock_{$key}"] ?? null;
    if (!is_resource($sock)) {
        echo 'raw-waiter-no-connection';
        return;
    }
    // Set before the write, which is the call that waits: nothing can be published
    // from inside it.
    $sharedState["rawwrite_waiting_{$key}"] = microtime(true);
    $written = @fwrite($sock, "ECHO b-probe\r\n");
    echo 'raw-waiter-done:' . var_export($written, true);
} catch (\Throwable $e) {
    echo 'raw-waiter-failed:' . str_replace("\n", ' ', $e->getMessage());
}
