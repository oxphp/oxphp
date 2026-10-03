<?php
/**
 * Prereq verification — OxPHP\Shared\Shareable interface is
 * registered at MINIT.
 *
 * This test does NOT instantiate a Shared\* class. It only verifies
 * through interface_exists and Reflection that the interface is
 * reachable and that a Shared type implements it.
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

// Verify a Shared type implements it. Not by declaring a class of our own:
// a class written in PHP is refused the interface.
if (!(new ReflectionClass('OxPHP\\Shared\\Counter'))->implementsInterface('OxPHP\\Shared\\Shareable')) {
    http_response_code(500);
    echo "FAIL: OxPHP\\Shared\\Counter does not implement Shareable\n";
    exit;
}

echo "OK: Shareable interface registered\n";
