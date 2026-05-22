<?php
// Reset mode (default): factory throw -> Uninitialized, retry succeeds.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;

$o = new Once();

$caught = false;
try {
    $o->getOrInit(function() { throw new \RuntimeException('boom'); });
} catch (\RuntimeException $e) {
    $caught = ($e->getMessage() === 'boom');
}
if (!$caught) { echo "FAIL: factory exception not propagated to caller\n"; exit; }
if ($o->status() !== Status::Uninitialized) { echo "FAIL: should reset to uninit\n"; exit; }

$v = $o->getOrInit(fn() => 777);
if ($v !== 777) { echo "FAIL: retry returned $v (want 777)\n"; exit; }
if ($o->status() !== Status::Ready) { echo "FAIL: ready after retry\n"; exit; }

echo "OK\n";
