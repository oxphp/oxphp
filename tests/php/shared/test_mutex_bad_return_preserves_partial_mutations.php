<?php
/**
 * Mutex::withLock / tryWithLock — when a closure that mutated the
 * by-reference state returns a non-Shareable value, the legitimate
 * partial mutation MUST survive (Mutex's no-rollback policy), and a
 * TypeException is raised pointing at the return value.
 *
 * This mirrors the PHP_THREW contract in
 * test_mutex_php_throw_not_poisoned.php: a closure doing
 * `$s = 11; throw …` leaves state at 11. By the same reasoning a
 * closure doing `$s = 11; return fn() => 1` (non-Shareable return)
 * must leave state at 11 — both are "closure ran to completion in a
 * non-rollback-able way", and the state mutation is independent of
 * the return value.
 *
 * Pre-fix the C-side invoker freed the successfully-serialised
 * partial state the moment the return value's serialisation failed,
 * so the mutation was silently dropped.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;
use OxPHP\Shared\TypeException;

// ── withLock: scalar mutation + non-Shareable return ───────────────
$m = new Mutex(0);
$threw = null;
try {
    $m->withLock(function (int &$s) {
        $s = 11; // Shareable mutation
        return fn() => 1; // closure — non-Shareable
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof TypeException)) {
    echo "FAIL: expected TypeException, got ",
         ($threw === null ? 'nothing' : $threw::class),
         "\n"; exit;
}
$observed = $m->withLock(fn(int &$s) => $s);
if ($observed !== 11) {
    echo "FAIL: partial mutation lost — expected 11, got $observed\n"; exit;
}

// ── tryWithLock: same contract on the non-blocking path ────────────
$m2 = new Mutex(0);
$threw = null;
try {
    $m2->tryWithLock(function (int &$s) {
        $s = 22;
        return new \stdClass(); // non-Shareable object
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof TypeException)) {
    echo "FAIL: tryWithLock expected TypeException, got ",
         ($threw === null ? 'nothing' : $threw::class),
         "\n"; exit;
}
$observed = $m2->withLock(fn(int &$s) => $s);
if ($observed !== 22) {
    echo "FAIL: tryWithLock partial mutation lost — expected 22, got $observed\n"; exit;
}

// ── Array state: structural mutations survive a bad return ─────────
$m3 = new Mutex(['n' => 0, 'tags' => []]);
$threw = null;
try {
    $m3->withLock(function (array &$s) {
        $s['n']++;
        $s['tags'][] = 'before-return';
        return fn() => 1; // non-Shareable
    });
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof TypeException)) {
    echo "FAIL: array case expected TypeException, got ",
         ($threw === null ? 'nothing' : $threw::class),
         "\n"; exit;
}
// Mutex::withLock currently returns only scalars; extract fields one-by-one.
$n_after = $m3->withLock(fn(array &$s) => $s['n']);
if ($n_after !== 1) {
    echo "FAIL: array['n'] mutation lost — expected 1, got ",
         var_export($n_after, true), "\n"; exit;
}
$tags_count = $m3->withLock(fn(array &$s) => count($s['tags']));
$tag0       = $m3->withLock(fn(array &$s) => $s['tags'][0] ?? null);
if ($tags_count !== 1 || $tag0 !== 'before-return') {
    echo "FAIL: array['tags'] mutation lost — count=$tags_count, first=",
         var_export($tag0, true), "\n"; exit;
}

// ── Mutex remains usable for normal acquires after a BAD_RETURN ────
$ok = $m->withLock(function (int &$s) { $s = 99; return $s; });
if ($ok !== 99) {
    echo "FAIL: mutex unusable after BAD_RETURN, got ", var_export($ok, true), "\n"; exit;
}
if ($m->withLock(fn(int &$s) => $s) !== 99) {
    echo "FAIL: subsequent acquire lost the post-recovery mutation\n"; exit;
}

echo "OK\n";
