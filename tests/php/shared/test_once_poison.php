<?php
// Poison mode: a failed factory poisons the cell terminally.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;
use OxPHP\Shared\Once\FailureMode;

$o = new Once(onFactoryError: FailureMode::Poison);

$caught = false;
try {
    $o->getOrInit(function() { throw new \RuntimeException('boom', 42); });
} catch (\RuntimeException $e) {
    // Current caller still receives the original exception.
    $caught = ($e->getMessage() === 'boom' && $e->getCode() === 42);
}
if (!$caught) { echo "FAIL: original exception not propagated\n"; exit; }

if ($o->status() !== Status::Poisoned) { echo "FAIL: should be poisoned\n"; exit; }

// status() must NOT throw on a poisoned cell — it is the safe observer.
if ($o->status() !== Status::Poisoned) { echo "FAIL: status() threw or wrong on poisoned\n"; exit; }

// Every value-access method throws PoisonedException carrying the original
// factory exception's code and message.
$threw = false;
try { $o->get(); } catch (OxPHP\Shared\PoisonedException $e) {
    $threw = ($e->getCode() === 42 && str_contains($e->getMessage(), 'boom'));
}
if (!$threw) { echo "FAIL: get() must throw PoisonedException with original code/message\n"; exit; }

$threw = false;
try { $o->getOrInit(fn() => 1); } catch (OxPHP\Shared\PoisonedException $e) { $threw = true; }
if (!$threw) { echo "FAIL: getOrInit() must throw PoisonedException\n"; exit; }

$threw = false;
try { $o->trySet(1); } catch (OxPHP\Shared\PoisonedException $e) { $threw = true; }
if (!$threw) { echo "FAIL: trySet() must throw PoisonedException\n"; exit; }

// Reset mode (default) does NOT poison — it stays retryable.
$r = new Once();
try { $r->getOrInit(function() { throw new \RuntimeException('x'); }); } catch (\RuntimeException $e) {}
if ($r->status() !== Status::Uninitialized) { echo "FAIL: reset mode must not poison\n"; exit; }

echo "OK\n";
