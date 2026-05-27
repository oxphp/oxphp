<?php
/**
 * Mutex::withLock no-rollback contract — BAD_RETURN must NOT silently
 * drop a state mutation whose serialised form contains an embedded
 * Shareable (e.g. a nested Shared\Map).
 *
 * Mechanism: the C invoker `oxphp_shared_invoke_byref_1_portbuf`
 *   1. serialises the by-ref state into `new_state_buf`  (embeds the
 *      Map's wire id), then
 *   2. tries to serialise the return value, fails (closure), returns
 *      `BAD_RETURN`, and BEFORE returning runs
 *      `zval_ptr_dtor(&state_zv)` — which drops the closure-local
 *      PHP wrapper for the embedded Map. If that wrapper was the only
 *      strong ref (created inside the closure, no outer alias), the
 *      `Arc<Entry>` strong count hits zero and the Entry is freed.
 *   3. Rust then receives the (still-valid) state bytes and runs
 *      `portbuf_to_sv(new_bytes).and_then(|raw| raw_to_owned(raw, ...))`.
 *      `raw_to_owned` tries to `Weak::upgrade` the registry slot for
 *      the embedded wire id and gets `None` → `Err`.
 *   4. The current code at `src/plugins/ox_shared/types/mutex.rs:342`
 *      does `if let Ok(new_sv) = … { *guard = new_sv; }`, silently
 *      swallowing the `Err`. *guard is not updated, but
 *      `Err(SharedError::Type)` is still returned to PHP and surfaces
 *      as a `TypeException` — the caller sees the exception and, per
 *      the documented no-rollback contract, expects the state mutation
 *      to have stuck. It did not.
 *
 * This test demonstrates the bug deterministically: the embedded
 * Map's only PHP-level ref is the closure-local by-ref state, so the
 * dtor always frees the Entry before Rust decodes.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;
use OxPHP\Shared\Map;
use OxPHP\Shared\TypeException;

$m = new Mutex(['marker' => 'initial']);

$threw = null;
try {
    $m->withLock(function (array &$s) {
        // Assigning a fresh Shared\Map into $s makes the by-ref state
        // the *only* PHP-level strong ref to the Map's Entry. After
        // the C dtor runs on state_zv, that ref is gone.
        $s = ['inner' => new Map()];
        $s['inner']->set('marker', 'survived-bad-return');
        return fn() => 1; // non-Shareable → BAD_RETURN
    });
} catch (\Throwable $e) {
    $threw = $e;
}

if (!($threw instanceof TypeException)) {
    echo "FAIL: expected TypeException, got ",
         ($threw === null ? 'nothing' : $threw::class),
         "\n"; exit;
}

// Per the no-rollback contract: state should now be the new array
// with the embedded Map. The bug: state silently rolls back to the
// initial ['marker' => 'initial'] because raw_to_owned failed and
// the `if let Ok` arm dropped the Err.
$has_inner = $m->withLock(fn(array &$s) =>
    isset($s['inner']) && is_object($s['inner']) && $s['inner'] instanceof Map
);
if (!$has_inner) {
    $observed = $m->withLock(fn(array &$s) =>
        isset($s['inner'])
            ? ('inner=' . gettype($s['inner']))
            : ('keys=[' . implode(',', array_keys($s)) . ']')
    );
    echo "FAIL: state mutation silently dropped — the closure assigned ",
         "['inner' => new Map()] but the by-ref state rolled back to its ",
         "initial value because the embedded Map's Entry was GC'd by the ",
         "C dtor before Rust's raw_to_owned could resolve the wire id. ",
         "Observed: $observed\n"; exit;
}

// And the Map's contents survived too (decoded under no-rollback).
$marker = $m->withLock(fn(array &$s) => $s['inner']->get('marker'));
if ($marker !== 'survived-bad-return') {
    echo "FAIL: nested Map's marker lost — got ",
         var_export($marker, true), "\n"; exit;
}

echo "OK\n";
