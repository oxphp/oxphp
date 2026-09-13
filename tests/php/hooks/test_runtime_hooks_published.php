<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// This profile runs with RUNTIME_HOOKS=1. Before the process published what it
// installed, that state was byte-identical from outside to a misspelled
// variable name and to RUNTIME_HOOKS=0: same `/config`, same server info, same
// output. What is pinned here is that the process answers with the set of
// categories it actually swapped handlers for.
//
// The spelling matters as much as the content. `1` is not a category name, so a
// field carrying `["sleep","streams"]` can only have come from the value being
// parsed into a state; a field echoing the string back would read `"1"` and
// leave every caller to re-implement the grammar to find out what that enables.

// Strict comparison throughout: `null == []` is true in PHP, so a loose
// assertion on a missing key would pass on exactly the build this test exists
// to fail.
$test = new TestCase('runtime_hooks_published', 'hooks');

$expected = ['sleep', 'streams'];

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
    'config reports runtime_hooks',
    array_key_exists('runtime_hooks', $decoded)
);
$test->assertSame(
    'config runtime_hooks is the parsed category set, not the raw value',
    $decoded['runtime_hooks'] ?? null,
    $expected
);

// The same answer from inside PHP, for an operator who has no reach to the
// internal port. One source, so the two cannot disagree.
$info = oxphp_server_info();
$test->assertTrue(
    'server info reports runtime_hooks',
    array_key_exists('runtime_hooks', $info)
);
$test->assertSame(
    'server info runtime_hooks matches /config',
    $info['runtime_hooks'] ?? null,
    $expected
);

$test->done();
