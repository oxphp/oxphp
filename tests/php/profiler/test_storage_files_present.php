<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('storage_files_present', 'profiler');

// Trigger a profile via the imperative SDK (no header needed).
OxPHP\Profile\start();
// Synthetic work — observer captures these calls under ProfileAll.
function _storage_inner(): int { return strlen("hello"); }
function _storage_outer(): int { return _storage_inner() + 1; }
_storage_outer();
OxPHP\Profile\stop();

// Sleep briefly to give the async tokio::spawn fan-out time to write.
usleep(300 * 1000);

$dir = '/tmp/oxphp-profiles';
$files = is_dir($dir) ? scandir($dir) : [];
$has_collapsed = false;
$has_index = false;
foreach ($files as $f) {
    if (str_ends_with($f, '.collapsed')) {
        $has_collapsed = true;
    }
    if ($f === 'index.json') {
        $has_index = true;
    }
}
$t->assertTrue("$dir contains at least one .collapsed file", $has_collapsed);
$t->assertTrue("$dir contains index.json", $has_index);

// Verify the index entry is well-formed JSON with at least the run_id.
if ($has_index) {
    $lines = file($dir . '/index.json', FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES);
    $first = $lines[0] ?? '';
    $entry = json_decode($first, true);
    $t->assertTrue('first index entry parses as JSON', is_array($entry));
    $t->assertTrue('entry has run_id', isset($entry['run_id']) && is_string($entry['run_id']));
    $t->assertTrue('entry has formats array', isset($entry['formats']) && is_array($entry['formats']));
}

$t->done();
