<?php

declare(strict_types=1);

// A compile that goes on after another compile on the same thread failed in the
// middle of it. The declaration eval'd below raises a deprecation while it is
// compiled, and the handler it reaches has a second compile fail: through
// opcache_compile_file(), which catches the compile error and returns false
// (?inner=opcache), or in another request the worker serves while this one is
// parked on the socket (?inner=request). The eval'd compile then runs to its end,
// where it destroys the syntax tree it built; the handler first takes the memory
// the failed compile gave back.
// See test_compile_error_during_a_compile_leaves_its_syntax_tree.php.

$inner = (string) ($_GET['inner'] ?? 'opcache');
$name = 'oxphp_nested_compile_' . bin2hex(random_bytes(4));
$filled = [];
$done = false;

set_error_handler(static function () use ($inner, &$filled, &$done): bool {
    if ($done) {
        return true;
    }
    $done = true;
    if ($inner === 'request') {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock !== false) {
            stream_set_timeout($sock, 10);
            fwrite($sock, "GET /tests/fibers/fixture_fatal.php?compile=1 HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            // Hooked: parks this request while the worker serves the inner one.
            $resp = (string) stream_get_contents($sock);
            fclose($sock);
            echo 'INNER:', str_contains($resp, 'OxphpProbeCompileFatal') ? 'fataled' : 'other', "\n";
        }
    } else {
        // Apart from the echo: the compile error is displayed while it runs.
        $compiled = opcache_compile_file(__DIR__ . '/fixture_compile_error.inc');
        echo 'INNER:', $compiled === false ? 'fataled' : 'other', "\n";
    }
    // 32 KiB apiece, the size of the arena the failed compile gave back. Not a
    // constant string: OPcache could build that once at compile time.
    for ($i = 0; $i < 8; $i++) {
        $filled[] = str_repeat((string) $i, 32768 - 25);
    }
    return true;
}, E_DEPRECATED);

eval("function {$name}(stdClass \$x = null): void {}");
restore_error_handler();

echo function_exists($name) ? "DECLARED\n" : "MISSING\n";
