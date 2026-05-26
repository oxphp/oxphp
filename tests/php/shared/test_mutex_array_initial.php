<?php
/**
 * Mutex::__construct(mixed $initial) — the stub declares `mixed`,
 * so arrays and objects must be either stored verbatim or rejected
 * with TypeException. Silently coercing to null is a data-loss bug.
 *
 * Reproducer for: handler's ValType match has a `_ => SharedValue::Null`
 * catch-all that drops every type except Long/Double/Bool/String.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;
use OxPHP\Shared\TypeException;

// ── 1. Array initial state ─────────────────────────────────────────
$threw = null;
$stored = null;
try {
    $m = new Mutex(['k' => 'v', 'n' => 42]);
    $stored = $m->withLock(fn(&$s) => $s);
} catch (\Throwable $e) {
    $threw = $e;
}

if ($threw !== null) {
    // Acceptable fix: reject with TypeException pointing at the initial arg.
    if (!($threw instanceof TypeException)) {
        echo "FAIL: array \$initial raised ", $threw::class,
             ", expected TypeException or successful round-trip. msg=",
             $threw->getMessage(), "\n"; exit;
    }
    if (!str_contains($threw->getMessage(), 'initial')
        && !str_contains($threw->getMessage(), 'array')) {
        echo "FAIL: TypeException for array \$initial must point at the initial arg, got: ",
             $threw->getMessage(), "\n"; exit;
    }
} else {
    // Acceptable fix: array stored verbatim and observable via withLock.
    if ($stored === null) {
        echo "FAIL: array \$initial silently coerced to null (data loss)\n"; exit;
    }
    if (!is_array($stored) || $stored !== ['k' => 'v', 'n' => 42]) {
        echo "FAIL: array \$initial round-trip lost data, got ", var_export($stored, true), "\n"; exit;
    }
}

// ── 2. Object initial state ────────────────────────────────────────
$threw = null;
$stored = null;
try {
    $o = new \stdClass();
    $o->x = 1;
    $m2 = new Mutex($o);
    $stored = $m2->withLock(fn(&$s) => $s);
} catch (\Throwable $e) {
    $threw = $e;
}

if ($threw === null && $stored === null) {
    echo "FAIL: object \$initial silently coerced to null (data loss)\n"; exit;
}
// Either a TypeException (preferred — plain stdClass is non-Shareable) or
// some non-null preserved form is acceptable; null without any error is not.
if ($threw !== null && !($threw instanceof TypeException)) {
    echo "FAIL: object \$initial raised ", $threw::class,
         ", expected TypeException. msg=", $threw->getMessage(), "\n"; exit;
}

echo "OK\n";
