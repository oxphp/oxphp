<?php
$w = OxPHP\Server\Worker::current();

/* getExitReason() typing: nullable string. */
$r = $w->getExitReason();
if ($r !== null && !is_string($r)) {
    http_response_code(500);
    echo "FAIL: getExitReason() must be ?string, got " . gettype($r) . "\n";
    exit;
}

/* Without scheduleExit() / memory cap hit / handler error, no exit pending. */
if ($r !== null) {
    http_response_code(500);
    echo "FAIL: unexpected pending exit reason: " . var_export($r, true) . "\n";
    exit;
}

/* Allowed values are limited to the documented mapping. */
$allowed = [null, 'scheduled', 'max_memory', 'error'];
if (!in_array($r, $allowed, true)) {
    http_response_code(500);
    echo "FAIL: getExitReason() returned a value outside of allowed set\n";
    exit;
}

echo "OK reason=" . var_export($r, true) . "\n";
