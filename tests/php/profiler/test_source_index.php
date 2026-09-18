<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Reads back the four runs the preceding source probes produced. This request
// carries no trigger and calls no start(), so it adds no run of its own.
//
// What it pins: the two facts the trigger knows when it admits a request — why
// profiling was turned on, and under which id the run will be filed — survive
// to the artefacts an operator reads. Both used to be discarded between the
// trigger and storage and replaced by constants: every run was filed as
// `Header`, and every run_id was a copy of the request id.

$t = new TestCase('source_index', 'profiler');

// The index entry is written from a tokio::spawn fan-out after the response is
// sent, so the last probe's line need not be on disk when this request starts.
// Poll for it rather than sleeping a fixed amount — same reasoning as
// profiler/test_truncated_index.
$dir = '/tmp/oxphp-profiles';
/** @var array<string, string> $probes probe script name => expected source */
$probes = [
    'header' => 'Header',
    'cookie' => 'Cookie',
    'query'  => 'Query',
    'sdk'    => 'Sdk',
];

/** @var array<string, array<string, mixed>> $runs keyed by the probe's short name */
$runs = [];
for ($attempt = 0; $attempt < 40; $attempt++) {
    $runs = [];
    $lines = is_file("$dir/index.json")
        ? file("$dir/index.json", FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES)
        : [];
    foreach ($lines as $line) {
        $entry = json_decode($line, true);
        if (!is_array($entry) || !isset($entry['url'])) {
            continue;
        }
        foreach (array_keys($probes) as $probe) {
            if (str_contains((string) $entry['url'], "test_source_$probe.php")) {
                $runs[$probe] = $entry;
            }
        }
    }
    if (count($runs) === count($probes)) {
        break;
    }
    usleep(50 * 1000);
}

foreach ($probes as $probe => $expected_source) {
    $t->assertTrue("index has the $probe run", isset($runs[$probe]));
}

// 1. Each run says how it was activated. Three of the four were filed as
//    "Header" before the trigger's decision reached storage; the header run is
//    the control that was right for the wrong reason.
foreach ($probes as $probe => $expected_source) {
    $t->assertSame("$probe run reports source $expected_source",
        $runs[$probe]['source'] ?? null, $expected_source);
}

// 2. Each run carries a run_id of the documented shape —
//    <ts_ms>-<request_id[:8]>-<rand4> — for every activation path, including
//    the one with no trigger to mint it.
foreach (array_keys($probes) as $probe) {
    $run_id = (string) ($runs[$probe]['run_id'] ?? '');
    $request_id = (string) ($runs[$probe]['request_id'] ?? '');
    $t->assertMatch("$probe run_id has the documented shape",
        $run_id, '/^\d{13,}-[0-9a-f]{8}-[0-9a-f]{4}$/');
    // The shape alone is satisfied by any well-formed id. These two tie it to
    // this request: the middle field is this run's own request id, and the
    // whole is not merely that id under a second name — which is exactly what
    // it was.
    $t->assertSame("$probe run_id embeds its own request id",
        explode('-', $run_id)[1] ?? null, substr($request_id, 0, 8));
    $t->assertTrue("$probe run_id is not a copy of the request id",
        $run_id !== '' && $run_id !== $request_id);
}

// 3. The same facts drive the counters behind /__profiler/stats and the
//    Prometheus runs_total series, which is what a dashboard splits by source.
$ch = curl_init('http://127.0.0.1:9090/__profiler/stats');
curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
curl_setopt($ch, CURLOPT_HTTPHEADER, ['Authorization: Bearer test-token']);
$body = (string) curl_exec($ch);
$code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
curl_close($ch);
$stats = json_decode($body, true);
$t->assertSame('stats 200', $code, 200);
$t->assertTrue('stats has a runs_total breakdown',
    is_array($stats) && isset($stats['runs_total']) && is_array($stats['runs_total']));
foreach (['header', 'cookie', 'query', 'sdk'] as $bucket) {
    $t->assertTrue("runs_total.$bucket counted at least one run",
        isset($stats['runs_total'][$bucket]) && $stats['runs_total'][$bucket] >= 1);
}

$t->done();
