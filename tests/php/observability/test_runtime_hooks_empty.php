<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The negative half of the pair, and the one the reporting exists for: this
// profile sets no RUNTIME_HOOKS at all, which is also what an operator gets
// from a misspelled variable name. The key must be present and empty rather
// than absent — an absent key says "this build cannot tell you", which reads
// the same on a server with hooks on and is the state being fixed.
//
// tests/php/hooks/test_runtime_hooks_published.php pins the other half.

// Strict comparison throughout: `null == []` is true in PHP, so a loose
// assertion on a missing key would pass on exactly the build this test exists
// to fail.
$test = new TestCase('runtime_hooks_empty', 'observability');

$config = @file_get_contents('http://127.0.0.1:9090/config');
$test->assertTrue('config endpoint reachable', is_string($config));

if (!is_string($config)) {
    $test->done();
    return;
}

$decoded = json_decode($config, true);
$test->assertTrue('config body is JSON', is_array($decoded));

if (!is_array($decoded)) {
    $test->done();
    return;
}

$test->assertTrue(
    'config reports runtime_hooks with no hooks configured',
    array_key_exists('runtime_hooks', $decoded)
);
$test->assertSame(
    'config runtime_hooks is empty',
    $decoded['runtime_hooks'] ?? null,
    []
);

$info = oxphp_server_info();
$test->assertTrue(
    'server info reports runtime_hooks',
    array_key_exists('runtime_hooks', $info)
);
$test->assertSame(
    'server info runtime_hooks is empty',
    $info['runtime_hooks'] ?? null,
    []
);

$test->done();
