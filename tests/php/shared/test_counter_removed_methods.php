<?php
/**
 * Defensive: the removed accumulator sugar is gone.
 *   inc/dec  → add($delta)
 *   addBatch → add(array_sum(...))
 *   reset    → set(0)
 * `swap` is also undefined as a PHP method — it stays internal (set() is
 * the public atomic exchange); this guards against it leaking back out.
 */

header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();

foreach (['inc', 'dec', 'addBatch', 'reset', 'swap'] as $method) {
    $threw = false;
    try {
        $c->{$method}(1);
    } catch (\Error $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: Counter::{$method} must be undefined\n"; exit; }
}

echo "OK\n";
