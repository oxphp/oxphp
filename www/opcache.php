<?php
$status = function_exists('opcache_get_status') ? opcache_get_status(false) : false;
$config = function_exists('opcache_get_configuration') ? opcache_get_configuration() : false;
$enabled = $status && !empty($status['opcache_enabled']);
$jit = $status['jit'] ?? null;

$mem = $status['memory_usage'] ?? [];
$stats = $status['opcache_statistics'] ?? [];

$mem_total = ($config['directives']['opcache.memory_consumption'] ?? 0);
$mem_used = ($mem['used_memory'] ?? 0) / 1048576;
$mem_free = ($mem['free_memory'] ?? 0) / 1048576;
$mem_pct = $mem_total > 0 ? round($mem_used / $mem_total * 100, 1) : 0;

$strings_buf = ($config['directives']['opcache.interned_strings_buffer'] ?? 0);
$strings_used = ($mem['used_memory_percentage'] ?? 0);

$max_files = $config['directives']['opcache.max_accelerated_files'] ?? 0;
$cached = $stats['num_cached_scripts'] ?? 0;
$hits = $stats['hits'] ?? 0;
$misses = $stats['misses'] ?? 0;
$hit_rate = ($hits + $misses) > 0 ? round($hits / ($hits + $misses) * 100, 1) : 0;

$jit_enabled = $jit['enabled'] ?? false;
$jit_kind = $jit['kind'] ?? 0;
$jit_buf_size = ($jit['buffer_size'] ?? 0) / 1048576;
$jit_buf_free = ($jit['buffer_free'] ?? 0) / 1048576;
$jit_buf_used = $jit_buf_size - $jit_buf_free;
$jit_buf_pct = $jit_buf_size > 0 ? round($jit_buf_used / $jit_buf_size * 100, 1) : 0;

function bar(float $pct, string $color = '#4f46e5'): string {
    $bg = $pct > 90 ? '#ef4444' : ($pct > 70 ? '#f59e0b' : $color);
    return '<div style="background:#e5e7eb;border-radius:6px;height:8px;margin-top:6px">'
         . '<div style="background:'.$bg.';width:'.$pct.'%;height:8px;border-radius:6px;transition:width .3s"></div>'
         . '</div>';
}

header('Content-Type: text/html; charset=utf-8');
header('Cache-Control: no-store');
?>
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>OxPHP — OPcache Status</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #f8fafc; color: #1e293b; line-height: 1.5; padding: 2rem 1rem; }
  .wrap { max-width: 720px; margin: 0 auto; }
  h1 { font-size: 1.5rem; font-weight: 700; margin-bottom: .25rem; }
  h1 span.ox { color: #b7472a; }
  h1 span.php { color: #777bb4; }
  .sub { color: #64748b; font-size: .85rem; margin-bottom: 1.5rem; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; margin-bottom: 1rem; }
  @media (max-width: 540px) { .grid { grid-template-columns: 1fr; } }
  .card { background: #fff; border: 1px solid #e2e8f0; border-radius: 12px; padding: 1.25rem; }
  .card.full { grid-column: 1 / -1; }
  .card h2 { font-size: .75rem; text-transform: uppercase; letter-spacing: .05em; color: #94a3b8; margin-bottom: .75rem; }
  .stat { font-size: 1.75rem; font-weight: 700; }
  .stat small { font-size: .875rem; font-weight: 400; color: #64748b; }
  .badge { display: inline-block; font-size: .7rem; font-weight: 600; padding: 2px 8px; border-radius: 9999px; }
  .badge.on { background: #dcfce7; color: #166534; }
  .badge.off { background: #fee2e2; color: #991b1b; }
  .row { display: flex; justify-content: space-between; align-items: baseline; padding: .4rem 0; border-bottom: 1px solid #f1f5f9; font-size: .85rem; }
  .row:last-child { border-bottom: none; }
  .row .label { color: #64748b; }
  .row .val { font-weight: 600; font-variant-numeric: tabular-nums; }
  .footer { text-align: center; margin-top: 2rem; color: #94a3b8; font-size: .75rem; }
</style>
</head>
<body>
<div class="wrap">
  <h1><span class="ox">Ox</span><span class="php">PHP</span> OPcache</h1>
  <div class="sub">SAPI: <?= php_sapi_name() ?> &middot; PHP <?= PHP_VERSION ?> ZTS &middot; <?= date('H:i:s') ?></div>

  <div class="grid">
    <div class="card">
      <h2>OPcache</h2>
      <span class="badge <?= $enabled ? 'on' : 'off' ?>"><?= $enabled ? 'Enabled' : 'Disabled' ?></span>
    </div>
    <div class="card">
      <h2>JIT</h2>
      <span class="badge <?= $jit_enabled ? 'on' : 'off' ?>"><?= $jit_enabled ? 'Enabled' : 'Disabled' ?></span>
      <?php if ($jit_enabled): ?>
        <span style="margin-left:.5rem;font-size:.8rem;color:#64748b">
          <?= $jit['opt_level'] ?? '' ?> / <?= ['off','tracing','function'][$jit_kind] ?? $jit_kind ?>
        </span>
      <?php endif; ?>
    </div>

    <div class="card">
      <h2>Memory</h2>
      <div class="stat"><?= round($mem_used, 1) ?> <small>/ <?= $mem_total ?> MB</small></div>
      <?= bar($mem_pct) ?>
    </div>
    <div class="card">
      <h2>JIT Buffer</h2>
      <div class="stat"><?= round($jit_buf_used, 1) ?> <small>/ <?= round($jit_buf_size) ?> MB</small></div>
      <?= bar($jit_buf_pct, '#8b5cf6') ?>
    </div>

    <div class="card">
      <h2>Hit Rate</h2>
      <div class="stat"><?= $hit_rate ?><small>%</small></div>
      <?= bar($hit_rate, '#10b981') ?>
    </div>
    <div class="card">
      <h2>Cached Scripts</h2>
      <div class="stat"><?= number_format($cached) ?> <small>/ <?= number_format($max_files) ?></small></div>
      <?= bar($max_files > 0 ? $cached / $max_files * 100 : 0, '#0ea5e9') ?>
    </div>

    <div class="card full">
      <h2>Details</h2>
      <div class="row"><span class="label">Cache hits</span><span class="val"><?= number_format($hits) ?></span></div>
      <div class="row"><span class="label">Cache misses</span><span class="val"><?= number_format($misses) ?></span></div>
      <div class="row"><span class="label">Interned strings buffer</span><span class="val"><?= $strings_buf ?> MB</span></div>
      <div class="row"><span class="label">validate_timestamps</span><span class="val"><?= ($config['directives']['opcache.validate_timestamps'] ?? 0) ? 'on' : 'off' ?></span></div>
      <div class="row"><span class="label">file_update_protection</span><span class="val"><?= $config['directives']['opcache.file_update_protection'] ?? '—' ?>s</span></div>
      <?php if ($jit_enabled): ?>
      <div class="row"><span class="label">JIT opt_flags</span><span class="val"><?= $jit['opt_flags'] ?? '—' ?></span></div>
      <?php endif; ?>
    </div>
  </div>

  <div class="footer">OxPHP v<?= getenv('CARGO_PKG_VERSION') ?: '0.1.0' ?></div>
</div>
</body>
</html>
