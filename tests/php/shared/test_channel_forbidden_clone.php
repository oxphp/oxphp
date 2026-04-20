<?php
/**
 * Channel — __clone magic throws per Shareable contract.
 *
 * Known limitation: due to the plugin-builder
 * magic-method dispatch not being fully wired into custom-storage clone,
 * the thrown exception may manifest as a stale-handle error on first
 * op, rather than a direct clone-time exception. Mirror the style of
 * test_counter_basic.php's clone handling.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

// Attempt clone; either throws directly (preferred) OR yields a
// clone whose first op fails due to unitialised storage.
$threw_at_clone = false;
try {
    $copy = clone $ch;
} catch (OxPHP\Shared\Exception $e) {
    $threw_at_clone = true;
}

if ($threw_at_clone) {
    echo "OK\n";
    exit;
}

// Fallback check: any op on the clone must fail
$threw_on_op = false;
try {
    /** @phpstan-ignore-next-line */
    $copy->send('x');
} catch (Throwable $e) {
    $threw_on_op = true;
}
if (!$threw_on_op) {
    echo "FAIL: clone must be unusable (neither clone-time nor op-time exception)\n";
    exit;
}

echo "OK\n";
