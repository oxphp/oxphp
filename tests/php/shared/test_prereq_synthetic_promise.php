<?php
/**
 * Prereq verification — synthetic promise FFI is wired.
 *
 * We verify indirectly: if the server started up, the
 * AsyncPlugin::init() call to synthetic::register_with_bridge()
 * succeeded (else MINIT would have panicked). We additionally
 * verify ox_async is enabled and functional, because any regression
 * in Part D would manifest there first.
 */

header('Content-Type: text/plain');

if (!function_exists('oxphp_async')) {
    http_response_code(500);
    echo "FAIL: oxphp_async() not available — AsyncPlugin did not init\n";
    exit;
}

// Dispatch a trivial closure via the real (non-synthetic) async path
// to prove the plugin hasn't regressed.
$promise = oxphp_async(fn() => 42);
$result = oxphp_async_await($promise, 1.0);

if ($result !== 42) {
    http_response_code(500);
    echo "FAIL: oxphp_async/await returned " . var_export($result, true) . ", expected 42\n";
    exit;
}

echo "OK: ox_async + synthetic bridge wired\n";
