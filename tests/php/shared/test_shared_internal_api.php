<?php
// Verify /__ox_shared/* routes and Prometheus metrics.
// Route shape: ?id= query param (not :id path segment — the plugin
// internal router doesn't support dynamic segments).

header('Content-Type: text/plain');

// Create sample entries so the API has data to report on.
$counter = new OxPHP\Shared\Counter(42);
$counter->add();
$counter->add();
$flag = new OxPHP\Shared\Flag(true);

$internal = 'http://127.0.0.1:9090';

$summary = @file_get_contents("$internal/__ox_shared/summary");
if ($summary === false) { echo "FAIL: summary fetch\n"; exit; }
$data = json_decode($summary, true);
if (!$data) { echo "FAIL: summary bad JSON\n"; exit; }
if (!isset($data['by_type']['Counter']['count'])) { echo "FAIL: no Counter count\n"; exit; }
if (!isset($data['by_type']['Flag']['count'])) { echo "FAIL: no Flag count\n"; exit; }
if (!isset($data['limits']['max_entries'])) { echo "FAIL: limits missing\n"; exit; }

$types_raw = @file_get_contents("$internal/__ox_shared/types");
if ($types_raw === false) { echo "FAIL: types fetch\n"; exit; }
$types = json_decode($types_raw, true);
if (!is_array($types['types']) || count($types['types']) !== 3) {
    echo "FAIL: expected 3 types, got " . (isset($types['types']) ? count($types['types']) : 'none') . "\n"; exit;
}

$entries_raw = @file_get_contents("$internal/__ox_shared/entries");
if ($entries_raw === false) { echo "FAIL: entries fetch\n"; exit; }
$entries = json_decode($entries_raw, true);
if (!is_array($entries['items']) || count($entries['items']) < 2) {
    echo "FAIL: fewer than 2 entries returned\n"; exit;
}
$first_id = $entries['items'][0]['id'];
if (!is_int($first_id) || $first_id < 1) { echo "FAIL: bad entry id\n"; exit; }

$entry_raw = @file_get_contents("$internal/__ox_shared/entry?id=$first_id");
if ($entry_raw === false) { echo "FAIL: entry fetch\n"; exit; }
$entry = json_decode($entry_raw, true);
if ($entry['id'] !== $first_id) { echo "FAIL: entry id mismatch\n"; exit; }
if (!isset($entry['type_specific'])) { echo "FAIL: no type_specific\n"; exit; }

$metrics = @file_get_contents("$internal/metrics");
if ($metrics === false) { echo "FAIL: metrics fetch\n"; exit; }
foreach ([
    'oxphp_shared_objects_total',
    'oxphp_shared_operations_total',
    'oxphp_shared_total_bytes',
    'oxphp_shared_capacity_saturation',
] as $m) {
    if (!str_contains($metrics, $m)) {
        echo "FAIL: metric $m missing\n"; exit;
    }
}

echo "OK\n";
