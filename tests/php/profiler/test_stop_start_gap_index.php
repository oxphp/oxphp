<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Reads back the run profiler/test_stop_start_gap produced. This request carries
// no trigger and calls no start(), so it adds no run of its own.

$t = new TestCase('stop_start_gap_index', 'profiler');

// The index entry is written after the response is sent, so the probe's line
// need not be on disk when this request starts.
$dir = '/tmp/oxphp-profiles';
$entry = null;
for ($attempt = 0; $attempt < 40 && $entry === null; $attempt++) {
    $lines = is_file("$dir/index.json")
        ? file("$dir/index.json", FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES)
        : [];
    foreach ($lines as $line) {
        $candidate = json_decode($line, true);
        if (is_array($candidate) && str_contains((string) ($candidate['url'] ?? ''), 'test_stop_start_gap.php')) {
            $entry = $candidate;
        }
    }
    if ($entry === null) {
        usleep(50 * 1000);
    }
}

$t->assertTrue('index has the run', $entry !== null);
$t->assertSame('no leaked spans', $entry['leaked_count'] ?? null, 0);

$collapsed = (string) @file_get_contents($dir . '/' . ($entry['run_id'] ?? '') . '.collapsed');
$t->assertTrue('call before the gap captured', str_contains($collapsed, '_gap_before'));
$t->assertTrue('call after the gap captured', str_contains($collapsed, '_gap_after'));
$t->assertFalse('call inside the gap not captured', str_contains($collapsed, '_gap_hidden'));

$t->done();
