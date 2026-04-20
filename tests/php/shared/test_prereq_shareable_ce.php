<?php
/**
 * Prereq verification — OxPHP\Shared\Shareable interface is
 * registered at MINIT.
 *
 * This test does NOT instantiate a Shared\* class. It only verifies
 * the interface is reachable and can be checked via instanceof and
 * interface_exists.
 */

header('Content-Type: text/plain');

$exists = interface_exists('OxPHP\\Shared\\Shareable', autoload: false);
if (!$exists) {
    http_response_code(500);
    echo "FAIL: OxPHP\\Shared\\Shareable interface not registered\n";
    exit;
}

// Reflection should see it and show it's user-facing empty.
$r = new ReflectionClass('OxPHP\\Shared\\Shareable');
if (!$r->isInterface()) {
    http_response_code(500);
    echo "FAIL: OxPHP\\Shared\\Shareable is not an interface\n";
    exit;
}

// Verify a class that implements it satisfies instanceof.
$anon = new class implements \OxPHP\Shared\Shareable {};
if (!($anon instanceof \OxPHP\Shared\Shareable)) {
    http_response_code(500);
    echo "FAIL: instanceof check failed\n";
    exit;
}

echo "OK: Shareable interface registered\n";
