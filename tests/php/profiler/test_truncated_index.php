<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Reads back the runs the four preceding probes produced. This request is
// not profiled itself (no start(), no trigger), so it adds no run of its own.
//
// What it pins: the span cap is reported all the way out to the artefacts
// operators read — the `truncated` field of an index.json entry and the
// oxphp_profiler_truncated_total counter behind /__profiler/stats — and it
// is reported for the run that hit the cap and for no other.

$t = new TestCase('truncated_index', 'profiler');

// The index entry is written from a tokio::spawn fan-out after the response
// is sent, so the last probe's line need not be on disk when this request
// starts. Poll for it instead of sleeping a fixed amount: the wait is a
// scheduling race with no known duration, and a fixed sleep either wastes
// it or is too short on a loaded machine. A run that never shows up is a
// real failure and the assertions below report it as one — the disk writer
// also sheds runs above PROFILER_DISK_MAX_PER_SEC and announces that only
// at WARN, which this profile's LOG_LEVEL drops, so the missing line is the
// only signal that reaches the test.
$dir = '/tmp/oxphp-profiles';
$probes = ['under_cap', 'deep_stack', 'over_cap', 'after_cap'];

/** @var array<string, array<string, mixed>> $runs keyed by the probe's script name */
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
        foreach ($probes as $probe) {
            if (str_contains((string) $entry['url'], "test_truncated_$probe.php")) {
                $runs[$probe] = $entry;
            }
        }
    }
    if (count($runs) === count($probes)) {
        break;
    }
    usleep(50 * 1000);
}

$t->assertTrue('index has the under-cap run', isset($runs['under_cap']));
$t->assertTrue('index has the deep-stack run', isset($runs['deep_stack']));
$t->assertTrue('index has the over-cap run', isset($runs['over_cap']));
$t->assertTrue('index has the after-cap run', isset($runs['after_cap']));

// The cap fired: the run says so, and its tree stops exactly at the cap.
$t->assertSame('over-cap run is truncated', $runs['over_cap']['truncated'] ?? null, true);
$t->assertSame('over-cap run stops at the cap', $runs['over_cap']['span_count'] ?? null, 500);

// And it fires for nothing else: a small run, and a run 40 frames deep —
// past the bridge's 32-entry open-stack mirror — are both untruncated.
$t->assertSame('under-cap run is not truncated', $runs['under_cap']['truncated'] ?? null, false);
$t->assertSame('deep-stack run is not truncated', $runs['deep_stack']['truncated'] ?? null, false);
// And it does not latch: the run that follows the truncated one on the same
// worker is clean again.
$t->assertSame('run after the truncated one is not truncated', $runs['after_cap']['truncated'] ?? null, false);
$t->assertTrue(
    'deep-stack run stayed under the cap',
    isset($runs['deep_stack']['span_count']) && $runs['deep_stack']['span_count'] < 500
);
// Untruncated is not the same as unharmed, and the difference is what
// makes the two flags separate. The eight frames past the 32-entry mirror
// emitted a BEGIN that no END ever matched, so finalize force-closed them
// and marked them leaked: they are in the tree, but their duration is not
// readable. The observer stamps a span's start from CLOCK_MONOTONIC while
// the force-close stamps its end from the wall clock, so these eight come
// out of the .collapsed export at ~1.79e15 us against single-digit
// microseconds for the frames that closed normally. Present but unusable
// is still not the same as absent, which is what the cap does. The shallow
// run shows none of it, which is what ties the leaks to the depth rather
// than to profiling in general.
// Exactly eight, not "at least": the mirror is pushed by open_depth and
// overflows only once all 32 slots are taken, so a 40-frame chain starting
// at mirror depth `base` overflows 40 - (32 - base) = 8 + base frames.
// Here `base` is 0 — this profile is Traditional mode, so the outermost
// PHP frame is the probe script's own top-level code, whose begin fired
// while the mode was still OFF and took no slot. Anything that holds a
// slot before the chain starts raises this number rather than lowering it:
// move the chain into a helper called after start(), or run the probe
// under worker mode where the script frame itself is observed, and the
// expected count is 9.
$t->assertSame('deep-stack run leaked the frames past the mirror',
    $runs['deep_stack']['leaked_count'] ?? null, 8);
$t->assertSame('under-cap run leaked nothing', $runs['under_cap']['leaked_count'] ?? null, 0);

// The same fact drives the metric that the documented alert rule watches.
$ch = curl_init('http://127.0.0.1:9090/__profiler/stats');
curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
curl_setopt($ch, CURLOPT_HTTPHEADER, ['Authorization: Bearer test-token']);
$body = (string) curl_exec($ch);
$code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
$stats = json_decode($body, true);
$t->assertSame('stats 200', $code, 200);
$t->assertTrue(
    'truncated_total counted the truncated run',
    is_array($stats) && isset($stats['truncated_total']) && $stats['truncated_total'] >= 1
);

$t->done();
