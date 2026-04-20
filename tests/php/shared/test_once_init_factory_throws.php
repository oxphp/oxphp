<?php
header('Content-Type: text/plain');

$o = new OxPHP\Shared\Once();

$caught = false;
try {
    $o->init(function() { throw new \RuntimeException('boom'); });
} catch (\RuntimeException $e) {
    $caught = ($e->getMessage() === 'boom');
}
if (!$caught) { echo "FAIL: RuntimeException not propagated\n"; exit; }
if ($o->isInitialized()) { echo "FAIL: should stay uninitialised after factory throw\n"; exit; }

$v = $o->init(fn() => 777);
if ($v !== 777) { echo "FAIL: retry returned $v (want 777)\n"; exit; }
if (!$o->isInitialized()) { echo "FAIL: should be initialised after successful retry\n"; exit; }

echo "OK\n";
