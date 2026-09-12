<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Reads back the runs the three preceding probes and their neighbours produced.
// This request is not profiled itself (no trigger), so it adds no run of its own.
//
// What it pins is the whole of what reaches an operator: each run in index.json
// holds the frames of the script named in its own `url`, its span_count counts
// its own calls, and `truncated` is raised for the run that hit the cap and for
// no other. An untriggered neighbour produces no run at all.

$t = new TestCase('runs_are_separate', 'profilerfiber');

$dir = '/tmp/oxphp-profiles';

// Each run is keyed by the script it profiled. The index entry is written from
// a spawned task after the probe's response is already on the wire, so the last
// one need not be on disk when this request starts. Polled rather than slept
// past: the wait is a scheduling race with no known duration. A run that never
// shows up is a real failure and the assertions below report it as one.
$wanted = [
    'outer_spans' => 'test_spans_stay_with_their_request.php',
    'inner_spans' => 'fixture_inner_profiled.php',
    'outer_cap'   => 'test_cap_stays_with_its_request.php',
    'inner_cap'   => 'fixture_inner_small.php',
    'outer_apm'   => 'test_apm_neighbour_leaves_profile_alone.php',
];

/** @var array<string, array<string, mixed>> $runs */
$runs = [];
/** @var list<array<string, mixed>> $all every entry the index holds */
$all = [];
for ($attempt = 0; $attempt < 60; $attempt++) {
    $runs = [];
    $all = [];
    $lines = is_file("$dir/index.json")
        ? file("$dir/index.json", FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES)
        : [];
    foreach ($lines as $line) {
        $entry = json_decode($line, true);
        if (!is_array($entry) || !isset($entry['url'])) {
            continue;
        }
        $all[] = $entry;
        foreach ($wanted as $key => $script) {
            if (str_contains((string) $entry['url'], $script)) {
                $runs[$key] = $entry;
            }
        }
    }
    if (count($runs) === count($wanted)) {
        break;
    }
    usleep(50 * 1000);
}

foreach ($wanted as $key => $script) {
    $t->assertTrue("index has a run for $script", isset($runs[$key]));
}

// The untriggered neighbour asked for nothing and must have got nothing. On a
// shared slot it inherits the parked request's mode and finalizes that
// request's tree under its own request id and url.
$plain = array_filter(
    $all,
    static fn(array $e): bool => str_contains((string) $e['url'], 'fixture_inner_plain.php')
);
$t->assertCount('the untriggered neighbour produced no run', $plain, 0);

// ── Whose frames each run holds ──────────────────────────────
//
// The collapsed export is one "frame;frame;frame count" line per leaf, so the
// name of a function that ran is in the file and the name of one that did not
// is not. Each probe calls a function nobody else calls, which is what makes
// this readable at all.
$collapsed = static function (array $run) use ($dir): string {
    $path = $dir . '/' . ($run['run_id'] ?? '') . '.collapsed';
    return is_file($path) ? (string) file_get_contents($path) : '';
};

$outerSpans = $collapsed($runs['outer_spans'] ?? []);
$innerSpans = $collapsed($runs['inner_spans'] ?? []);

$t->assertContains('the parked request kept its own frames', $outerSpans, 'pf_outer_fn');
$t->assertNotContains('the parked request took none of the neighbour\'s frames',
    $outerSpans, 'pf_inner_fn');
$t->assertContains('the neighbour kept its own frames', $innerSpans, 'pf_inner_fn');
$t->assertNotContains('the neighbour took none of the parked request\'s frames',
    $innerSpans, 'pf_outer_fn');

$outerApm = $collapsed($runs['outer_apm'] ?? []);
$t->assertContains('the request that parked across untriggered traffic kept its frames',
    $outerApm, 'pf_third_fn');
$t->assertNotContains('and took none of that traffic\'s frames', $outerApm, 'pf_plain_fn');

// Every frame each run holds was closed by its own return. A span the profiler
// had to close itself at the end of the request arrives `leaked`, and on a
// shared open-frame mirror that is what a neighbour's outermost calls become:
// the mirror still holds the parked request's frames underneath them, so the
// neighbour's returns find someone else's entry on top and pop nothing.
foreach ($wanted as $key => $script) {
    $t->assertSame("no frame of $script was left for the profiler to close",
        $runs[$key]['leaked_count'] ?? null, 0);
}

// ── Whose calls each run counted, and whose cap it reports ───
//
// Both halves of the budget guarantee, because either alone is satisfied by the
// defect: the request that burned past the cap is capped and says so, and the
// small request that ran while it was parked is neither.
$t->assertSame('the burning run stops at the cap', $runs['outer_cap']['span_count'] ?? null, 500);
$t->assertSame('the burning run says it was truncated', $runs['outer_cap']['truncated'] ?? null, true);

$t->assertGreaterThan('the small run recorded its own calls',
    $runs['inner_cap']['span_count'] ?? 0, 0);
$t->assertLessThan('the small run is nowhere near the cap',
    $runs['inner_cap']['span_count'] ?? 500, 100);
$t->assertSame('the small run is not truncated', $runs['inner_cap']['truncated'] ?? null, false);

// And the flag is on the one run that earned it and on no other. `inner_cap`
// is the run that follows the burning one in time and is covered above; these
// three ran before it. Together they are every run this thread produced, which
// is what makes `truncated` a property of a request rather than of the thread.
$t->assertSame('the first probe is not truncated',
    $runs['outer_spans']['truncated'] ?? null, false);
$t->assertSame('the neighbour of the first probe is not truncated',
    $runs['inner_spans']['truncated'] ?? null, false);
$t->assertSame('the request that parked across untriggered traffic is not truncated',
    $runs['outer_apm']['truncated'] ?? null, false);

$t->done();
