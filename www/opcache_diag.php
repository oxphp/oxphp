<?php
header('Content-Type: text/plain');

echo "SAPI: " . php_sapi_name() . "\n";
echo "opcache loaded: " . (extension_loaded('Zend OPcache') ? 'yes' : 'no') . "\n";
echo "opcache_get_status: ";
$s = opcache_get_status(false);
if ($s === false) {
    echo "FALSE (disabled or not available)\n";
} else {
    echo "OK\n";
    echo "  enabled: " . ($s['opcache_enabled'] ? 'yes' : 'no') . "\n";
    echo "  cache_full: " . ($s['cache_full'] ? 'yes' : 'no') . "\n";
    echo "  restart_pending: " . ($s['restart_pending'] ? 'yes' : 'no') . "\n";
    echo "  restart_in_progress: " . ($s['restart_in_progress'] ? 'yes' : 'no') . "\n";
    echo "  cached_scripts: " . $s['opcache_statistics']['num_cached_scripts'] . "\n";
    echo "  cached_keys: " . $s['opcache_statistics']['num_cached_keys'] . "\n";
    echo "  hits: " . $s['opcache_statistics']['hits'] . "\n";
    echo "  misses: " . $s['opcache_statistics']['misses'] . "\n";
    echo "  blacklist_misses: " . $s['opcache_statistics']['blacklist_misses'] . "\n";
    echo "  oom_restarts: " . $s['opcache_statistics']['oom_restarts'] . "\n";
    echo "  hash_restarts: " . $s['opcache_statistics']['hash_restarts'] . "\n";
    echo "  used_memory: " . round($s['memory_usage']['used_memory']/1048576, 1) . "MB\n";
    echo "  free_memory: " . round($s['memory_usage']['free_memory']/1048576, 1) . "MB\n";
    echo "  scripts: " . count($s['scripts'] ?? []) . "\n";
}

echo "\nopcache_get_configuration: ";
$c = opcache_get_configuration();
if ($c === false) {
    echo "FALSE\n";
} else {
    echo "OK\n";
    $d = $c['directives'];
    echo "  enable: " . var_export($d['opcache.enable'], true) . "\n";
    echo "  enable_cli: " . var_export($d['opcache.enable_cli'], true) . "\n";
    echo "  validate_timestamps: " . var_export($d['opcache.validate_timestamps'], true) . "\n";
    echo "  file_update_protection: " . $d['opcache.file_update_protection'] . "\n";
    echo "  use_cwd: " . var_export($d['opcache.use_cwd'], true) . "\n";
    echo "  file_cache: " . var_export($d['opcache.file_cache'], true) . "\n";
    echo "  file_cache_only: " . var_export($d['opcache.file_cache_only'], true) . "\n";
    echo "  memory: " . $d['opcache.memory_consumption'] . "\n";
    echo "  max_files: " . $d['opcache.max_accelerated_files'] . "\n";
    echo "  jit: " . $d['opcache.jit'] . "\n";
    echo "  jit_buffer: " . $d['opcache.jit_buffer_size'] . "\n";
}

echo "\nopcache_compile_file test: ";
$result = opcache_compile_file(__FILE__);
echo ($result ? 'OK' : 'FAILED') . "\n";

echo "opcache_is_script_cached: ";
echo (opcache_is_script_cached(__FILE__) ? 'yes' : 'no') . "\n";
