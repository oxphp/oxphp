<?php
/**
 * Reproducer: Mutex::withLock / tryWithLock closure returning a freshly-
 * constructed Shared\* wrapper (whose only strong ref is the return zval
 * itself) silently degrades to null on the PHP side.
 *
 * Mechanism: ext/bridge/oxphp_bridge.c (oxphp_shared_invoke_byref_1_portbuf)
 * serialises ret_zv into a wire id, then `zval_ptr_dtor(&ret_zv)` drops the
 * sole wrapper → Arc<Entry> hits zero → registry's Weak goes stale before
 * Rust (mutex.rs invoke_mutex_with) calls `raw_to_owned`. raw_to_owned
 * yields StaleHandle, the match arm calls `call.ret_null()`, and the PHP
 * caller sees null instead of the Shared\* the closure returned.
 *
 * The state-side path was fixed in this branch via `out_retained_state`
 * (ZVAL_COPY pin) — see test_mutex_shareable_in_state_ok_path.php. The
 * symmetric retention for the return value is missing; invoke_0_portbuf
 * (used by Once/Registry factories) does have it via out_retained_entry.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;
use OxPHP\Shared\Counter;

// ── 1. withLock returns a fresh Counter (no outer references) ──────────
$m = new Mutex(0);

$ret = $m->withLock(function (&$s) {
    // The Counter is constructed inside the closure and returned as the
    // sole strong ref. No assignment to $s, no outer capture.
    return new Counter();
});

if (!is_object($ret)) {
    echo "FAIL: withLock returned non-object: ", var_export($ret, true), "\n";
    exit;
}
if (!($ret instanceof Counter)) {
    echo "FAIL: withLock returned ", get_class($ret), " instead of Counter\n";
    exit;
}
// The returned Counter must be a live, usable Shared handle — not a
// freshly-allocated empty husk with a dangling id.
if ($ret->get() !== 0)              { echo "FAIL: returned Counter::get() !== 0\n"; exit; }
if ($ret->add(7) !== 7)             { echo "FAIL: returned Counter::add(7) !== 7\n"; exit; }
if ($ret->get() !== 7)              { echo "FAIL: returned Counter state did not persist\n"; exit; }

// ── 2. tryWithLock has the same path — confirm symmetric repro ─────────
$m2 = new Mutex(0);

$ret2 = $m2->tryWithLock(function (&$s) {
    return new Counter();
});

if (!is_object($ret2)) {
    echo "FAIL: tryWithLock returned non-object: ", var_export($ret2, true), "\n";
    exit;
}
if (!($ret2 instanceof Counter)) {
    echo "FAIL: tryWithLock returned ", get_class($ret2), " instead of Counter\n";
    exit;
}
if ($ret2->get() !== 0)             { echo "FAIL: tryWithLock Counter::get() !== 0\n"; exit; }
if ($ret2->add(3) !== 3)            { echo "FAIL: tryWithLock Counter::add(3) !== 3\n"; exit; }

// ── 3. Returning a Shared\* that DOES have an outer reference must
//      still work (control: state retention or outer-scope pin keeps
//      Entry alive across zval_ptr_dtor). Establishes that the bug is
//      specifically about the ref-count reaching zero, not a general
//      decoder failure. ────────────────────────────────────────────────
$pinned = new Counter();
$pinned->add(42);

$m3 = new Mutex(0);
$ret3 = $m3->withLock(function (&$s) use ($pinned) {
    // $pinned has an extra ref from the `use`-captured closure binding,
    // so even if ret_zv is dropped, the Entry survives.
    return $pinned;
});

if (!($ret3 instanceof Counter) || $ret3->get() !== 42) {
    echo "FAIL: control case (outer-pinned Counter) regressed — got ",
         var_export($ret3, true), "\n";
    exit;
}

echo "OK\n";
