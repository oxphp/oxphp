<?php
/**
 * Map — __clone must throw, matching every other Shared type.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();
$m->set('k', 1);

$threw = false;
try {
    $cloned = clone $m;
} catch (OxPHP\Shared\SharedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: clone must throw Shared\\SharedException\n"; exit; }

// Original still works.
if ($m->get('k') !== 1) { echo "FAIL: original damaged by clone attempt\n"; exit; }

echo "OK\n";
