<?php
/**
 * Defensive: confirm Counter no longer exposes swap or compareAndSet
 * (atomic-leaked methods migrated to Shared\Atomic).
 */

header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();

$threw = false;
try {
    $c->swap(0);
} catch (\Error $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: Counter::swap must be undefined\n"; exit; }

$threw = false;
try {
    $c->compareAndSet(0, 1);
} catch (\Error $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: Counter::compareAndSet must be undefined\n"; exit; }

echo "OK\n";
