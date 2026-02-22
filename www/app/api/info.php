<?php

json_response(200, [
    'server'     => $_SERVER['SERVER_SOFTWARE'] ?? 'unknown',
    'sapi'       => php_sapi_name(),
    'php'        => PHP_VERSION,
    'zts'        => PHP_ZTS,
    'os'         => PHP_OS,
    'arch'       => php_uname('m'),
    'pid'        => getmypid(),
    'executor'   => getenv('EXECUTOR') ?: 'sapi',
    'workers'    => getenv('PHP_WORKERS') ?: 'auto',
    'opcache'    => function_exists('opcache_get_status') && opcache_get_status() !== false,
    'extensions' => get_loaded_extensions(),
    'timestamp'  => time(),
]);
