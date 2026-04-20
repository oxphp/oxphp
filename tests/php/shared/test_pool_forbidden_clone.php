<?php
/**
 * Pool — cloning Shared\Pool and Shared\Pool\Handle is forbidden,
 * matching every other Shared\* type.
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn() => new stdClass(),
    null, // destroy
    2,    // maxSize
);

$threwPool = false;
try {
    $clone = clone $pool;
} catch (\OxPHP\Shared\Exception $e) {
    $threwPool = true;
}
if (!$threwPool) { echo "FAIL: clone Pool must throw Shared\\Exception\n"; exit; }

$h = $pool->acquire();

$threwHandle = false;
try {
    $clone = clone $h;
} catch (\OxPHP\Shared\Exception $e) {
    $threwHandle = true;
}
if (!$threwHandle) { echo "FAIL: clone Handle must throw Shared\\Exception\n"; exit; }

$pool->release($h);

echo "OK\n";
