<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The default profile has no .oxphpdeny: /config still carries the key, with
// nothing but `loaded`, so a scraper can tell "no file" from "old build".
$t = new TestCase('test_config_without_file', 'oxphpdeny');

$config = @file_get_contents('http://127.0.0.1:9090/config');
$decoded = is_string($config) ? json_decode($config, true) : null;
$t->assertSame('config deny_file reports no file', $decoded['deny_file'] ?? null, ['loaded' => false]);
$t->done();
