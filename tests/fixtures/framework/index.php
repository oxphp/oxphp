<?php
// In the framework test profile this file is mounted at the document root
// (/var/www/html/public/index.php) and ./php is mounted at ./tests, so the
// shared helper lives at __DIR__/tests/test_helper.php.
require __DIR__ . '/tests/test_helper.php';

// Front controller for the Framework-mode test profile. Every request is
// rewritten here, so this fixture validates the standard server-var contract
// for whatever path it was reached by. It is document-root-agnostic: it only
// requires SCRIPT_NAME to end in `/index.php`, then derives the expected
// PATH_INFO from REQUEST_URI relative to the actual SCRIPT_NAME.
$t = new TestCase('framework_index', 'routing');

$scriptName = $_SERVER['SCRIPT_NAME'] ?? '';
// REQUEST_URI is raw (percent-encoded); SCRIPT_NAME / PATH_INFO are decoded.
// Decode the path before comparing so an encoded explicit-entry request
// (e.g. /index.php/u%20ser) derives the expected PATH_INFO correctly.
$path = rawurldecode(parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH) ?? '');

$t->assertNotEqual('SCRIPT_NAME is not empty', $scriptName, '');
$t->assertTrue('SCRIPT_NAME ends with /index.php', str_ends_with($scriptName, '/index.php'));

if ($scriptName !== '' && $path === $scriptName) {
    // Bare entry: /index.php -> no PATH_INFO.
    $t->assertKeyMissing('PATH_INFO absent for bare entry', $_SERVER, 'PATH_INFO');
} elseif ($scriptName !== '' && str_starts_with($path, $scriptName . '/')) {
    // Explicit entry with trailing segment: /index.php/news -> /news.
    $t->assertSame(
        'PATH_INFO is tail after entry',
        $_SERVER['PATH_INFO'] ?? '',
        substr($path, strlen($scriptName))
    );
} else {
    // App route (/users/42): rewritten to the front controller, no PATH_INFO.
    $t->assertKeyMissing('PATH_INFO absent for app route', $_SERVER, 'PATH_INFO');
}

$t->done();
