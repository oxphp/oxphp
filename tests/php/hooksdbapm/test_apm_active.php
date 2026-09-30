<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('apm_active', 'hooksdbapm');

// Every other test in this profile is a hooksdb test run a second time, and each
// of them passes just as well on a process where APM never came up — a misspelled
// variable is enough. So this checks, from `/config`, that the APM plugin is
// enabled and the streams runtime hooks are installed.
//
// It does not show that the APM wrapper itself went in: `hooks_registered` is
// counted when the plugin initialises, before module startup validates those
// targets and records their handlers, so it stays positive even if none of them
// ends up wrapped. The tests that follow fail when the wrapper calls around the
// runtime hook, but pass when there is no wrapper at all, and nothing here tells
// those two apart.
$config = @file_get_contents('http://127.0.0.1:9090/config');
$t->assertTrue('config endpoint reachable', is_string($config));
$decoded = is_string($config) ? json_decode($config, true) : null;
$t->assertTrue('config body is JSON', is_array($decoded));

if (!is_array($decoded)) {
    $t->done();
    return;
}

$t->assertSame(
    'the database runtime hooks are installed',
    in_array('streams', $decoded['runtime_hooks'] ?? [], true),
    true
);

$apm = $decoded['plugins']['apm'] ?? [];
$t->assertSame('the APM plugin is enabled', $apm['enabled'] ?? null, true);
$t->assertGreaterThan(
    'the APM plugin registered function hooks to be installed',
    (int) ($apm['hooks_registered'] ?? 0),
    0
);

$t->done();
