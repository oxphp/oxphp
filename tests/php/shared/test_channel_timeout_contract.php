<?php
/**
 * Channel — int $ms timeout contract:
 *   recvTimeout / sendTimeout require `int $ms > 0`.
 *   Zero, negative, and non-int input throw TypeException.
 *
 * Forever / try semantics moved to dedicated methods (recv / tryRecv /
 * send / trySend) and are exercised by their own tests.
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

// ms = 0 → TypeException on recvTimeout.
$caught = null;
try {
    $ch->recvTimeout(0);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: recvTimeout(0) must throw TypeException\n"; exit; }

// ms < 0 → TypeException on recvTimeout.
$caught = null;
try {
    $ch->recvTimeout(-1);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: recvTimeout(-1) must throw TypeException\n"; exit; }

// ms = 0 → TypeException on sendTimeout.
$caught = null;
try {
    $ch->sendTimeout('x', 0);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: sendTimeout(\$v, 0) must throw TypeException\n"; exit; }

// ms < 0 → TypeException on sendTimeout.
$caught = null;
try {
    $ch->sendTimeout('x', -5);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: sendTimeout(\$v, -5) must throw TypeException\n"; exit; }

// Non-int (float) → TypeException — the bridge enforces `int $ms` itself
// rather than relying on the engine's parameter coercion.
$caught = null;
try {
    /** @phpstan-ignore-next-line */
    $ch->recvTimeout(0.5);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: recvTimeout(0.5) must throw TypeException\n"; exit; }

echo "OK\n";
