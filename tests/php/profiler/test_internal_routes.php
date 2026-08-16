<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('internal_routes', 'profiler');

// Trigger a profile via the imperative SDK to ensure at least one run
// is captured before we probe the internal endpoints.
OxPHP\Profile\start();
function _ir_leaf(): int { return 1 + 1; }
function _ir_outer(): int { return _ir_leaf() + _ir_leaf(); }
_ir_outer();
OxPHP\Profile\stop();
// Give the async tokio::spawn fan-out time to land the index + files on disk.
usleep(300 * 1000);

$auth = 'test-token';
$base = 'http://127.0.0.1:9090/__profiler';

/**
 * @param array{method?: string} $opts
 * @return array{code: int, head: string, body: string}
 */
function curl_internal(string $url, array $opts = [], ?string $auth = null): array {
    $ch = curl_init($url);
    curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
    curl_setopt($ch, CURLOPT_HEADER, true);
    if (isset($opts['method'])) {
        curl_setopt($ch, CURLOPT_CUSTOMREQUEST, $opts['method']);
    }
    $hdrs = [];
    if ($auth !== null) { $hdrs[] = 'Authorization: Bearer ' . $auth; }
    if ($hdrs) { curl_setopt($ch, CURLOPT_HTTPHEADER, $hdrs); }
    $raw = curl_exec($ch);
    $code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
    $hsize = (int) curl_getinfo($ch, CURLINFO_HEADER_SIZE);
    return [
        'code' => $code,
        'head' => substr((string)$raw, 0, $hsize),
        'body' => substr((string)$raw, $hsize),
    ];
}

// /__profiler/stats
$r = curl_internal("$base/stats", [], $auth);
$t->assertSame('stats 200', $r['code'], 200);
$stats = json_decode($r['body'], true);
$t->assertTrue('stats has spans_collected_total', is_array($stats) && isset($stats['spans_collected_total']));
$t->assertTrue('spans_collected_total >= 1', is_array($stats) && $stats['spans_collected_total'] >= 1);

// /__profiler/runs
$r = curl_internal("$base/runs?limit=5", [], $auth);
$t->assertSame('runs 200', $r['code'], 200);
$data = json_decode($r['body'], true);
$t->assertTrue('runs is array', is_array($data) && isset($data['runs']));
$t->assertTrue('at least one run', is_array($data) && count($data['runs']) >= 1);
$run_id = $data['runs'][0]['run_id'] ?? null;
$t->assertTrue('run has run_id', is_string($run_id));

// /__profiler/runs/{id}
$r = curl_internal("$base/runs/$run_id", [], $auth);
$t->assertSame('run metadata 200', $r['code'], 200);
$meta = json_decode($r['body'], true);
$t->assertTrue('metadata has run_id', is_array($meta) && isset($meta['run_id']) && $meta['run_id'] === $run_id);

// /__profiler/runs/{id}.collapsed
$r = curl_internal("$base/runs/$run_id.collapsed", [], $auth);
$t->assertSame('collapsed 200', $r['code'], 200);
$t->assertTrue('collapsed non-empty', strlen($r['body']) > 0);

// /__profiler/runs/{id}/speedscope -> 302 to speedscope.app
// Hyper emits header names in lowercase, so match case-insensitively.
$r = curl_internal("$base/runs/$run_id/speedscope", [], $auth);
$t->assertSame('speedscope 302', $r['code'], 302);
$t->assertTrue('Location to speedscope.app',
    stripos($r['head'], 'location: https://www.speedscope.app/#profileURL=') !== false
);

// Auth enforcement
$r = curl_internal("$base/stats");
$t->assertSame('stats no auth 401', $r['code'], 401);
$r = curl_internal("$base/stats", [], 'wrong-token');
$t->assertSame('stats wrong auth 401', $r['code'], 401);

// /metrics exposes oxphp_profiler_* lines from the registered collector
$r = curl_internal('http://127.0.0.1:9090/metrics');
$t->assertSame('metrics 200', $r['code'], 200);
$t->assertTrue('metrics contains oxphp_profiler_runs_total',
    str_contains($r['body'], 'oxphp_profiler_runs_total')
);

// DELETE removes the run + index entry
$r = curl_internal("$base/runs/$run_id", ['method' => 'DELETE'], $auth);
$t->assertSame('delete 204', $r['code'], 204);
$r = curl_internal("$base/runs/$run_id", [], $auth);
$t->assertSame('deleted run gone', $r['code'], 404);

$t->done();
