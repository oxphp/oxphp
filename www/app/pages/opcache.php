<?php

$status  = function_exists('opcache_get_status') ? opcache_get_status(false) : false;
$config  = function_exists('opcache_get_configuration') ? opcache_get_configuration() : false;
$enabled = $status && !empty($status['opcache_enabled']);

if (!$enabled) {
    layout('OPcache', '<div class="card"><div class="card-body"><p>OPcache is not available or disabled.</p></div></div>');
    return;
}

$mem  = $status['memory_usage'];
$stat = $status['opcache_statistics'];
$jit  = $status['jit'] ?? null;

$used_mb  = round($mem['used_memory'] / 1048576, 1);
$free_mb  = round($mem['free_memory'] / 1048576, 1);
$total_mb = round(($mem['used_memory'] + $mem['free_memory']) / 1048576, 1);
$mem_pct  = $total_mb > 0 ? round($used_mb / $total_mb * 100, 1) : 0;

$hits     = $stat['hits'];
$misses   = $stat['misses'];
$hit_rate = ($hits + $misses) > 0 ? round($hits / ($hits + $misses) * 100, 1) : 0;
$scripts  = $stat['num_cached_scripts'];
$keys     = $stat['num_cached_keys'];
$max_keys = $stat['max_cached_keys'];

$jit_html = '';
if ($jit) {
    $jit_enabled = !empty($jit['enabled']) ? 'Yes' : 'No';
    $jit_on      = !empty($jit['on']) ? 'Yes' : 'No';
    $jit_kind    = $jit['opt_level'] ?? '-';
    $jit_buf     = isset($jit['buffer_size']) ? round($jit['buffer_size'] / 1048576, 1) . ' MB' : '-';
    $jit_free    = isset($jit['buffer_free']) ? round($jit['buffer_free'] / 1048576, 1) . ' MB' : '-';
    $jit_html = <<<HTML
    <div class="card">
        <div class="card-header">JIT</div>
        <div class="card-body">
            <table class="table-kv">
                <tr><td>Enabled</td><td>{$jit_enabled}</td></tr>
                <tr><td>Active</td><td>{$jit_on}</td></tr>
                <tr><td>Opt Level</td><td>{$jit_kind}</td></tr>
                <tr><td>Buffer</td><td>{$jit_buf}</td></tr>
                <tr><td>Free</td><td>{$jit_free}</td></tr>
            </table>
        </div>
    </div>
    HTML;
}

$directives_html = '';
if ($config) {
    $d = $config['directives'];
    $important = [
        'opcache.enable', 'opcache.enable_cli', 'opcache.memory_consumption',
        'opcache.max_accelerated_files', 'opcache.validate_timestamps',
        'opcache.revalidate_freq', 'opcache.jit', 'opcache.jit_buffer_size',
        'opcache.file_update_protection', 'opcache.file_cache',
    ];
    $rows = '';
    foreach ($important as $key) {
        if (isset($d[$key])) {
            $val = is_bool($d[$key]) ? ($d[$key] ? 'true' : 'false') : (string)$d[$key];
            $rows .= "<tr><td class=\"mono\">{$key}</td><td class=\"mono\">{$val}</td></tr>";
        }
    }
    $directives_html = <<<HTML
    <div class="card">
        <div class="card-header">Configuration</div>
        <div class="card-body"><table class="table-kv">{$rows}</table></div>
    </div>
    HTML;
}

layout('OPcache', <<<HTML
<div class="grid-2">
    <div class="card">
        <div class="card-header">Memory</div>
        <div class="card-body">
            <div class="progress-bar"><div class="progress-fill" style="width:{$mem_pct}%"></div></div>
            <p class="small mono">{$used_mb} MB / {$total_mb} MB ({$mem_pct}%)</p>
        </div>
    </div>
    <div class="card">
        <div class="card-header">Cache</div>
        <div class="card-body">
            <table class="table-kv">
                <tr><td>Hit Rate</td><td>{$hit_rate}%</td></tr>
                <tr><td>Hits</td><td>{$hits}</td></tr>
                <tr><td>Misses</td><td>{$misses}</td></tr>
                <tr><td>Scripts</td><td>{$scripts}</td></tr>
                <tr><td>Keys</td><td>{$keys} / {$max_keys}</td></tr>
            </table>
        </div>
    </div>
</div>
{$jit_html}
{$directives_html}
HTML);
