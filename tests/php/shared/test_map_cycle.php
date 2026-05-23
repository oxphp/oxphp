<?php
/**
 * Map — cycle detection on write.
 */

header('Content-Type: text/plain');

$a = new OxPHP\Shared\Map();
$b = new OxPHP\Shared\Map();

// a -> b is allowed.
$a->set('b', $b);

// b -> a closes a cycle.
$threw = false;
try {
    $b->set('a', $a);
} catch (OxPHP\Shared\CycleException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: cycle must throw CycleException\n"; exit; }
if ($b->get('a') !== null) { echo "FAIL: b must not carry the rejected edge\n"; exit; }

// CycleException extends TypeException per spec 05-exceptions.md.
$threw = false;
try {
    $b->set('a', $a);
} catch (OxPHP\Shared\TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: CycleException must be catchable as TypeException\n"; exit; }

// Self-insert is always rejected.
$m = new OxPHP\Shared\Map();
$threw = false;
try {
    $m->set('self', $m);
} catch (OxPHP\Shared\CycleException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: self-insert must throw\n"; exit; }

// Cycle via nested array reference.
$x = new OxPHP\Shared\Map();
$y = new OxPHP\Shared\Map();
$x->set('y', $y);
$threw = false;
try {
    $y->set('nested', ['reachable' => $x]);
} catch (OxPHP\Shared\CycleException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: nested-array cycle must throw\n"; exit; }
if ($y->get('nested') !== null) { echo "FAIL: y must not store rejected array\n"; exit; }

echo "OK\n";
