<?php
/**
 * Defensive: the old Event-style API is gone.
 *   isSet    → load()
 *   set      → store(true)  (or swap(true) for the prior value)
 *   clear    → store(false) (or swap(false) for the prior value)
 *   exchange → swap()
 */

header('Content-Type: text/plain');

$f = new OxPHP\Shared\Flag();

foreach (['isSet', 'set', 'clear', 'exchange'] as $method) {
    $threw = false;
    try {
        $f->{$method}(true);
    } catch (\Error $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: Flag::{$method} must be undefined\n"; exit; }
}

echo "OK\n";
