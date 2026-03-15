<?php
/**
 * OxPHP Demo Application — Front Controller
 *
 * Routing mode: Framework (INDEX_FILE=index.php)
 * All application code lives in ../app/ — outside DOCUMENT_ROOT.
 */

define('APP_ROOT', dirname(__DIR__) . '/app');

require APP_ROOT . '/helpers.php';

$path = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);

match (true) {
    // HTML pages
    $path === '/'          => require APP_ROOT . '/pages/dashboard.php',
    $path === '/echo'      => require APP_ROOT . '/pages/echo.php',
    $path === '/upload'    => require APP_ROOT . '/pages/upload.php',
    $path === '/cookies'   => require APP_ROOT . '/pages/cookies.php',
    $path === '/opcache'   => require APP_ROOT . '/pages/opcache.php',
    $path === '/functions' => require APP_ROOT . '/pages/functions.php',
    $path === '/sse'       => require APP_ROOT . '/pages/sse.php',
    $path === '/async'     => require APP_ROOT . '/pages/async.php',

    // JSON API
    str_starts_with($path, '/api/echo')       => require APP_ROOT . '/api/echo.php',
    $path === '/api/search'                   => require APP_ROOT . '/api/search.php',
    $path === '/api/upload'                   => require APP_ROOT . '/api/upload.php',
    str_starts_with($path, '/api/cookies')    => require APP_ROOT . '/api/cookies.php',
    $path === '/api/slow'                     => require APP_ROOT . '/api/slow.php',
    $path === '/api/async'                    => require APP_ROOT . '/api/async.php',
    str_starts_with($path, '/api/sse-native') => require APP_ROOT . '/api/sse_native.php',
    str_starts_with($path, '/api/sse')        => require APP_ROOT . '/api/sse.php',
    $path === '/api/error'                    => require APP_ROOT . '/api/error.php',
    $path === '/api/large'                    => require APP_ROOT . '/api/large.php',
    $path === '/api/headers'                  => require APP_ROOT . '/api/headers.php',
    $path === '/api/info'                     => require APP_ROOT . '/api/info.php',

    // Fallback
    default => json_response(404, ['error' => 'Not Found', 'path' => $_SERVER['REQUEST_URI']]),
};
