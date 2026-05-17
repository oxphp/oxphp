<?php
/**
 * Mutex — a PHP throw inside withLock / tryWithLock / withLockTimeout
 * propagates the closure's exception verbatim (class + message) without
 * the framework overwriting it with a generic RuntimeException, and
 * leaves the mutex usable for subsequent acquires with the partial
 * mutation observable.
 *
 * The previous isPoisoned() accessor is gone; the equivalent assertion
 * is "next withLock still succeeds and sees the partial mutation".
 *
 * Uses a distinctive exception class + message so that a regression
 * which silently throws Rust-side RuntimeException("Mutex::withLock
 * closure threw") instead of letting the closure's own exception pass
 * through would fail this test loudly. (Without the distinctive
 * payload, the previous version of this file caught \RuntimeException
 * — which matched the regression too, hiding the bug.)
 */
header('Content-Type: text/plain');

function assert_closure_throw_propagates(
    string $method,
    callable $invoke,
    OxPHP\Shared\Mutex $m,
    int $expected_partial,
): void {
    $caught = null;
    try {
        $invoke();
        echo "FAIL: $method must have thrown\n"; exit;
    } catch (\LogicException $e) {
        $caught = $e;
    } catch (\Throwable $e) {
        echo "FAIL: $method propagated wrong type: ", $e::class, " message=", $e->getMessage(), "\n"; exit;
    }
    if ($caught->getMessage() !== "from closure: $method") {
        echo "FAIL: $method propagated wrong message: ", var_export($caught->getMessage(), true), "\n"; exit;
    }
    if ($m->withLock(fn(int &$s) => $s) !== $expected_partial) {
        echo "FAIL: $method — partial mutation lost (expected $expected_partial)\n"; exit;
    }
}

// ── withLock ───────────────────────────────────────────────────────
$m = new OxPHP\Shared\Mutex(0);
assert_closure_throw_propagates('withLock', function () use ($m) {
    $m->withLock(function (int &$s) {
        $s = 11;
        throw new \LogicException('from closure: withLock');
    });
}, $m, 11);

// ── withLockTimeout ────────────────────────────────────────────────
$m2 = new OxPHP\Shared\Mutex(0);
assert_closure_throw_propagates('withLockTimeout', function () use ($m2) {
    $m2->withLockTimeout(function (int &$s) {
        $s = 22;
        throw new \LogicException('from closure: withLockTimeout');
    }, ms: 5000);
}, $m2, 22);

// ── tryWithLock (lock is free, so we DO enter the closure) ─────────
$m3 = new OxPHP\Shared\Mutex(0);
assert_closure_throw_propagates('tryWithLock', function () use ($m3) {
    $m3->tryWithLock(function (int &$s) {
        $s = 33;
        throw new \LogicException('from closure: tryWithLock');
    });
}, $m3, 33);

echo "OK\n";
