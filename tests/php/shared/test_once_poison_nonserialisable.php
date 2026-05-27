<?php
/**
 * Once + FailureMode::Poison: a factory that ran to completion but
 * produced a non-serialisable value (closure / resource / non-Shareable
 * object) is treated as a "factory failure" — same operational category
 * as a thrown exception — and poisons the cell terminally.
 *
 * Previously NotSerialisable always reset the cell to Uninitialized
 * regardless of FailureMode, silently violating the documented Poison
 * contract ("a failed factory is terminally Poisoned").
 *
 * In Reset mode the cell stays retryable — same as today.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;
use OxPHP\Shared\Once\FailureMode;

// ── Poison mode: non-serialisable return must POISON the cell ──────
$o = new Once(onFactoryError: FailureMode::Poison);

// First caller sees TypeException explaining what went wrong.
$threw = null;
try {
    $o->getOrInit(fn() => fn() => 1); // returns a closure → not Shareable
} catch (\Throwable $e) {
    $threw = $e;
}
if (!($threw instanceof OxPHP\Shared\TypeException)) {
    echo "FAIL: first caller must receive TypeException, got: ",
         ($threw === null ? 'nothing' : $threw::class), "\n"; exit;
}

// Cell is now Poisoned (terminal).
if ($o->status() !== Status::Poisoned) {
    echo "FAIL: cell must be Poisoned after non-serialisable factory under Poison mode, got: ",
         $o->status()->name, "\n"; exit;
}

// Subsequent get() must raise PoisonedException carrying an explanatory
// message that names TypeException as the original cause.
$threw = null;
try { $o->get(); } catch (\Throwable $e) { $threw = $e; }
if (!($threw instanceof OxPHP\Shared\PoisonedException)) {
    echo "FAIL: get() on poisoned cell must throw PoisonedException, got: ",
         ($threw === null ? 'nothing' : $threw::class), "\n"; exit;
}
if (!str_contains($threw->getMessage(), 'TypeException')) {
    echo "FAIL: PoisonedException message must reference TypeException, got: ",
         $threw->getMessage(), "\n"; exit;
}

// Subsequent getOrInit() with a perfectly valid factory must also
// throw PoisonedException — Poison is terminal, no retry.
$threw = null;
try { $o->getOrInit(fn() => 42); } catch (\Throwable $e) { $threw = $e; }
if (!($threw instanceof OxPHP\Shared\PoisonedException)) {
    echo "FAIL: getOrInit() on poisoned cell must throw PoisonedException, got: ",
         ($threw === null ? 'nothing' : $threw::class), "\n"; exit;
}

// trySet() too.
$threw = null;
try { $o->trySet(1); } catch (\Throwable $e) { $threw = $e; }
if (!($threw instanceof OxPHP\Shared\PoisonedException)) {
    echo "FAIL: trySet() on poisoned cell must throw PoisonedException\n"; exit;
}

// ── Reset mode (default): non-serialisable return stays RETRYABLE ──
$r = new Once();
try {
    $r->getOrInit(fn() => fopen('php://memory', 'r+')); // resource → not Shareable
} catch (OxPHP\Shared\TypeException) {
    // expected
}
if ($r->status() !== Status::Uninitialized) {
    echo "FAIL: Reset mode must leave cell Uninitialized after non-serialisable factory, got: ",
         $r->status()->name, "\n"; exit;
}
// Retry with a good factory must succeed.
$v = $r->getOrInit(fn() => 'recovered');
if ($v !== 'recovered') { echo "FAIL: Reset mode retry must succeed\n"; exit; }
if ($r->status() !== Status::Ready) { echo "FAIL: cell must be Ready after recovery\n"; exit; }

echo "OK\n";
