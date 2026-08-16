<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_result_refcount', 'async');

const PAYLOAD_LEN = 128;

/**
 * Refcount of the refcounted value stored under $key, measured through an
 * identical array fetch for every case so the numbers are comparable.
 * The fetch itself adds one reference for the duration of the dump, which is
 * why the assertions compare against a baseline instead of a fixed number.
 */
function probe_refcount(array $holder, string|int $key): int
{
    ob_start();
    debug_zval_dump($holder[$key]);
    $dump = (string) ob_get_clean();

    return preg_match('/refcount\((\d+)\)/', $dump, $m) === 1 ? (int) $m[1] : -1;
}

function payload(): string
{
    return str_repeat('x', PAYLOAD_LEN);
}

function spawn(): int
{
    return oxphp_async(static fn(int $n): string => str_repeat('x', $n), PAYLOAD_LEN);
}

// ── Baseline: a plain string held by exactly one array ───────────────────────

$baseline = probe_refcount(['value' => payload()], 'value');
$t->assertGreaterThan('probe parses a refcount', $baseline, 0);
$t->meta('baseline', $baseline);

// ── Control: single await writes straight into the return slot ───────────────

$single_value = oxphp_async_await(spawn());
$holder = ['value' => $single_value];
unset($single_value);
$single = probe_refcount($holder, 'value');
$t->meta('single', $single);
$t->assertSame('await: result holds no extra reference', $single, $baseline);

// ── await_all ────────────────────────────────────────────────────────────────

$ids = [spawn(), spawn(), spawn()];
$all = oxphp_async_await_all($ids);
$all_counts = [];
foreach ($ids as $id) {
    $all_counts[] = probe_refcount($all, $id);
}
$t->meta('await_all', $all_counts);
$t->assertSame(
    'await_all: no result holds an extra reference',
    $all_counts,
    array_fill(0, count($ids), $baseline)
);
$t->assertSame('await_all: values intact', array_values($all), array_fill(0, count($ids), payload()));

// ── await_race ───────────────────────────────────────────────────────────────
// Raced over several promises, not one: with more than one member the handler
// also has to put the non-winners back, which is the shape real callers use.

$race = oxphp_async_await_race([spawn(), spawn(), spawn()]);
$race_count = probe_refcount($race, 'value');
$t->meta('await_race', $race_count);
$t->assertSame('await_race: winner holds no extra reference', $race_count, $baseline);
$t->assertSame('await_race: value intact', $race['value'], payload());

// ── await_any ────────────────────────────────────────────────────────────────

$any = oxphp_async_await_any([spawn(), spawn(), spawn()]);
$any_count = probe_refcount($any, 'value');
$t->meta('await_any', $any_count);
$t->assertSame('await_any: winner holds no extra reference', $any_count, $baseline);
$t->assertSame('await_any: value intact', $any['value'], payload());

$t->done();
