<?php
/**
 * Empty key is argument validation, not a domain type error → SPL
 * \InvalidArgumentException (root namespace), distinct from
 * Shared\TypeException.
 *
 * Also covers: Registry is non-instantiable.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;
use OxPHP\Shared\SharedException;

header('Content-Type: text/plain');

// ── Empty key on typed method ──
$threw = false;
try {
    Registry::map('', fn() => new Map());
} catch (\InvalidArgumentException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: empty key must throw \\InvalidArgumentException\n"; exit; }

// ── Empty key on global() ──
$threw = false;
try {
    Registry::global('', fn() => new Map());
} catch (\InvalidArgumentException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: empty key on global() must throw \\InvalidArgumentException\n"; exit; }

// ── Empty key on remove() ──
$threw = false;
try {
    Registry::remove('');
} catch (\InvalidArgumentException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: empty key on remove() must throw \\InvalidArgumentException\n"; exit; }

// ── Non-instantiable: `new Registry()` throws. ──
$threw = false;
try {
    new Registry();
} catch (SharedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: 'new Registry()' must throw — class is a static facade\n"; exit; }

echo "OK\n";
