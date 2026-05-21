<?php
/**
 * Pool::tryAcquire() — non-blocking acquire. Fills up to maxSize,
 * returns null when saturated, and returns a Handle again once a slot
 * is released.
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, 2);

// The declared return type must be nullable (?Handle): the saturated path
// returns null, so a non-nullable `object` arginfo would lie about the
// contract (and trip return-type checks under a ZEND_DEBUG build).
$tryRt = (new ReflectionMethod(OxPHP\Shared\Pool::class, 'tryAcquire'))->getReturnType();
if ($tryRt === null || !$tryRt->allowsNull()) {
    echo "FAIL: tryAcquire() return type must be nullable (?Handle)\n"; exit;
}
// Contrast: acquire() never returns null, so its return type must NOT be nullable.
$acqRt = (new ReflectionMethod(OxPHP\Shared\Pool::class, 'acquire'))->getReturnType();
if ($acqRt === null || $acqRt->allowsNull()) {
    echo "FAIL: acquire() return type must be non-nullable\n"; exit;
}

$a = $pool->tryAcquire();
$b = $pool->tryAcquire();
if (!$a instanceof OxPHP\Shared\Pool\Handle || !$b instanceof OxPHP\Shared\Pool\Handle) {
    echo "FAIL: tryAcquire must fill up to maxSize\n"; exit;
}

$c = $pool->tryAcquire(); // pool now saturated
if ($c !== null) { echo "FAIL: tryAcquire past maxSize must return null\n"; exit; }

$a->release();
$d = $pool->tryAcquire(); // a slot freed
if (!$d instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: tryAcquire after release must return Handle\n"; exit; }

$b->release();
$d->release();

echo "OK\n";
