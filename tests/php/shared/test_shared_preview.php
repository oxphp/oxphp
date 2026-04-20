<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex("hello world");
$id = $m->id();

$raw = @file_get_contents("http://127.0.0.1:9090/__ox_shared/preview?id=$id");
if ($raw === false) { echo "FAIL: preview fetch\n"; exit; }
$data = json_decode($raw, true);
if (!$data || !isset($data['preview'])) { echo "FAIL: no preview field\n"; exit; }
if (!str_contains($data['preview'], 'hello')) {
    echo "FAIL: preview content: " . var_export($data['preview'], true) . "\n"; exit;
}

$ctx = stream_context_create(['http' => ['ignore_errors' => true]]);
$bad = file_get_contents("http://127.0.0.1:9090/__ox_shared/preview?id=999999", false, $ctx);
$bad_json = json_decode($bad, true);
if (!isset($bad_json['error'])) {
    echo "FAIL: expected error payload for missing id\n"; exit;
}

$metrics = @file_get_contents("http://127.0.0.1:9090/metrics");
if (!str_contains($metrics, 'oxphp_shared_deadlock_detected_total')) {
    echo "FAIL: deadlock metric missing\n"; exit;
}

echo "OK\n";
