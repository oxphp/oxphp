<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// SHARED_ENABLED=false, end to end: the half of the contract that lives on
// the internal server. Routes, the Prometheus collector and the published
// config are three separate registrations in the plugin, and an operator
// meets all three — a dashboard that keeps scraping `oxphp_shared_*` after
// the switch is thrown is the symptom this pins.
$t = new TestCase('endpoints_absent', 'shared_off');

$internal = 'http://127.0.0.1:9090';

// Status codes, not `false` returns: `file_get_contents` on a 404 returns
// false too, and so does an internal server that never came up. Reading the
// code separates "the route is gone" from "nothing answered". curl rather
// than a stream context because `$http_response_header` is deprecated as of
// PHP 8.5 and this suite has to stay quiet on both supported versions.
$status = static function (string $url): ?int {
    $ch = curl_init($url);
    curl_setopt_array($ch, [
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => 5,
    ]);
    $ok = curl_exec($ch) !== false;
    $code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
    return $ok ? $code : null;
};

// The three collection routes only. The `?id=`-keyed ones — entry, preview,
// graph — answer 404 for an id that does not exist, so on the enabled build
// they 404 too: an assertion on them is green whether or not the route is
// registered, which is worth nothing here.
foreach ([
    '/__ox_shared/summary',
    '/__ox_shared/entries',
    '/__ox_shared/types',
] as $path) {
    $t->assertSame("$path answers 404", $status($internal . $path), 404);
}

// The internal server itself is up — otherwise every 404 above would be
// reporting the server's absence rather than the routes'.
$t->assertSame('the internal server is serving', $status($internal . '/health'), 200);

// No metrics collector means no `oxphp_shared_*` family anywhere in the
// exposition, including the deprecated aliases the enabled build still emits.
$metrics = @file_get_contents($internal . '/metrics');
$t->assertTrue('metrics endpoint reachable', is_string($metrics));
if (is_string($metrics)) {
    $t->assertNotContains('no oxphp_shared_ metrics are exported', $metrics, 'oxphp_shared_');
    // A guard on the guard: an empty or truncated body would satisfy the
    // assertion above without proving anything.
    $t->assertContains('metrics body is a real exposition', $metrics, 'oxphp_requests_total');
}

$config = @file_get_contents($internal . '/config');
$t->assertTrue('config endpoint reachable', is_string($config));
$decoded = is_string($config) ? json_decode($config, true) : null;
$t->assertTrue('config body is JSON', is_array($decoded));

if (is_array($decoded)) {
    // The plugin still reports itself — "off" is a state it publishes, not
    // silence. The exact-equality check is the point: `enabled` alone means
    // `init` returned before `max_entries`/`max_bytes`, which are exposed
    // from the same function a few lines further down.
    $plugins = $decoded['plugins'] ?? [];
    $t->assertKeyExists('config lists the ox_shared plugin', $plugins, 'ox_shared');
    $t->assertSame(
        'ox_shared publishes exactly {enabled: false}',
        $plugins['ox_shared'] ?? null,
        ['enabled' => false]
    );
}

$t->done();
