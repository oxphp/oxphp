<?php

$path   = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);
$action = match ($path) {
    '/api/cookies/set'   => 'set',
    '/api/cookies/clear' => 'clear',
    default              => 'get',
};

if ($action === 'set') {
    $name  = $_GET['name'] ?? 'demo';
    $value = $_GET['value'] ?? ('oxphp-' . time());
    setcookie($name, $value, [
        'expires'  => time() + 3600,
        'path'     => '/',
        'secure'   => false,
        'httponly' => false,
        'samesite' => 'Lax',
    ]);
    json_response(200, ['action' => 'set', 'name' => $name, 'value' => $value]);
} elseif ($action === 'clear') {
    foreach ($_COOKIE as $name => $val) {
        setcookie($name, '', ['expires' => 1, 'path' => '/']);
    }
    json_response(200, ['action' => 'clear', 'cleared' => array_keys($_COOKIE)]);
} else {
    json_response(200, ['cookies' => $_COOKIE]);
}
