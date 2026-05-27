<?php
/**
 * Mutex::withLock / tryWithLock — a closure that ran to completion but
 * either returned a non-serialisable value (closure, resource,
 * non-Shareable object) OR mutated the by-reference state into one
 * surfaces a TypeException whose message points at the return / state,
 * NOT a generic "arg is not a valid callable".
 *
 * Before the BAD_RETURN split was extended to the by-reference invoker,
 * both of these conditions reported "Mutex::with: arg is not a valid
 * callable" — actively misleading: the callable was fine, the value
 * was the problem.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;

// ── 1. withLock: closure returns a non-Shareable value (a closure) ─
$m = new Mutex(0);
$threw = null;
try {
    $m->withLock(function (int &$s) {
        return fn() => 1; // closures are not Shareable
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if ($threw === null) {
    echo "FAIL: withLock with closure return must throw\n"; exit;
}
if (!($threw instanceof OxPHP\Shared\TypeException)) {
    echo "FAIL: withLock with closure return raised ", $threw::class,
         " — expected TypeException. msg=", $threw->getMessage(), "\n"; exit;
}
if (!str_contains($threw->getMessage(), 'non-serialisable')) {
    echo "FAIL: withLock TypeException message must point at the return value, got: ",
         $threw->getMessage(), "\n"; exit;
}

// State must NOT be mutated by a failed serialise.
if ($m->withLock(fn(int &$s) => $s) !== 0) {
    echo "FAIL: state must be unchanged after non-serialisable return\n"; exit;
}

// ── 2. withLock: closure assigns a non-Shareable INTO the state ────
$m2 = new Mutex(0);
$threw = null;
try {
    $m2->withLock(function (mixed &$s) {
        $s = fopen('php://memory', 'r+'); // resources are not Shareable
        return null;
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof OxPHP\Shared\TypeException)) {
    echo "FAIL: withLock with non-Shareable state assignment raised ",
         ($threw === null ? 'nothing' : $threw::class), "\n"; exit;
}
if (!str_contains($threw->getMessage(), 'non-serialisable')) {
    echo "FAIL: withLock state-assignment TypeException must point at the value, got: ",
         $threw->getMessage(), "\n"; exit;
}
// State preserved.
if ($m2->withLock(fn(int &$s) => $s) !== 0) {
    echo "FAIL: state must be unchanged after non-serialisable assignment\n"; exit;
}

// ── 3. tryWithLock: same diagnosis on the non-blocking path ────────
$m3 = new Mutex(0);
$threw = null;
try {
    $m3->tryWithLock(function (int &$s) {
        return new \stdClass(); // not Shareable
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof OxPHP\Shared\TypeException)) {
    echo "FAIL: tryWithLock with non-Shareable return raised ",
         ($threw === null ? 'nothing' : $threw::class), "\n"; exit;
}
if (!str_contains($threw->getMessage(), 'non-serialisable')) {
    echo "FAIL: tryWithLock TypeException message must point at the return, got: ",
         $threw->getMessage(), "\n"; exit;
}

// ── 4. The mutex must remain USABLE after each failure ─────────────
if ($m->withLock(function (int &$s) { $s = 7; return $s; }) !== 7) {
    echo "FAIL: mutex unusable after non-serialisable failure\n"; exit;
}
if ($m->withLock(fn(int &$s) => $s) !== 7) {
    echo "FAIL: subsequent acquire lost the post-recovery mutation\n"; exit;
}

echo "OK\n";
