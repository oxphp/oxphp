<?php
/**
 * OPcache Preload Script
 *
 * Compiles and caches files at server startup so they're never read from
 * disk during request handling. Enable in php.ini:
 *
 *   opcache.preload = /var/www/html/preload.php
 *   opcache.preload_user = www-data
 *
 * Files loaded here stay in shared memory for the lifetime of the process.
 */

$root = __DIR__;

// Front controller + helpers — loaded on every request
opcache_compile_file("{$root}/public/index.php");
opcache_compile_file("{$root}/app/helpers.php");

// Pages
foreach (glob("{$root}/app/pages/*.php") as $file) {
    opcache_compile_file($file);
}

// API handlers
foreach (glob("{$root}/app/api/*.php") as $file) {
    opcache_compile_file($file);
}
