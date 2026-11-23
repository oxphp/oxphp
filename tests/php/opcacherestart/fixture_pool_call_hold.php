<?php

declare(strict_types=1);

use OxPHP\Shared\Map;
use OxPHP\Shared\Registry;

// For refused_pool_call_does_not_hold_the_restart: acquires from the pool under
// `builtin` in the map named by `key`, whose factory a pool thread registered
// and whose call this thread makes, then holds on until `<base>.go2` appears.

require_once __DIR__ . '/lib.php';

$key = (string) ($_GET['key'] ?? '');
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_poolcall';
$map = Registry::map($key, static fn (): Map => new Map());
$pool = $map->get('builtin');

try {
    $h = $pool->acquire();
    $got = get_class($h->get());
    $h->release();
} catch (\Throwable $e) {
    $got = $e::class . ': ' . $e->getMessage();
}
$mine = ox_opcache_state();
file_put_contents("$base.held", json_encode(['got' => $got]));

$until = microtime(true) + 20.0;
while (!file_exists("$base.go2") && microtime(true) < $until) {
    usleep(20000);
    clearstatcache();
}

header('Content-Type: application/json');
echo json_encode(['got' => $got, 'opcache_enabled' => $mine['opcache_enabled']]);
