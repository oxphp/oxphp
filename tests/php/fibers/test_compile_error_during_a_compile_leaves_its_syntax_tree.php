<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An error handler reached in the middle of a compile can have another compile
// on the same thread fail, and the first one then goes on: opcache_compile_file()
// catches the compile error and returns false, and a request parked in the
// handler resumes after the worker has served one that failed to compile. The
// compile that goes on destroys its syntax tree when it ends, so the failed one
// must leave it pointing at that tree, not at its own, whose memory has been
// given back and handed out again by then. Pointing at the wrong one, it frees
// what other code owns, and the server goes down with it.

// Null when the inner request could not be sent.
$get = static function (string $path): ?string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    // Hooked: parks this request while the worker serves the inner one.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    return $resp;
};

$t = new TestCase('compile_error_during_a_compile_leaves_its_syntax_tree', 'fibers');

foreach (['opcache', 'request'] as $inner) {
    $resp = $get("fixture_compile_nested_bailout.php?inner={$inner}");
    // A request that ends normally, so that the fatals here and in the tests
    // around this one do not add up to having the worker replaced.
    $next = $get('fixture_fatal.php');

    $t->assertTrue("{$inner}: the request connected", $resp !== null);
    $t->assertContains("{$inner}: the compile in the handler failed", (string) $resp, 'INNER:fataled');
    $t->assertContains("{$inner}: and the one it interrupted declared its function", (string) $resp, 'DECLARED');
    $t->assertContains("{$inner}: the worker serves the next request", (string) $next, 'FRAMES:');
}

$t->done();
