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
} catch (\OxPHP\Shared\SharedException $e) {
    $threwPool = true;
}
if (!$threwPool) { echo "FAIL: clone Pool must throw Shared\\SharedException\n"; exit; }

$h = $pool->acquire();

$threwHandle = false;
try {
    $clone = clone $h;
} catch (\OxPHP\Shared\SharedException $e) {
    $threwHandle = true;
}
if (!$threwHandle) { echo "FAIL: clone Handle must throw Shared\\SharedException\n"; exit; }

$h->release();

echo "OK\n";
