<?php
// Holds a worker for ?ms= milliseconds the way an application does: running
// PHP, not sleeping in a C call.
//
// pause.php's usleep() cannot be interrupted — the engine only delivers a
// cancellation at an opcode boundary, and there are none inside usleep(), so a
// request aborted mid-sleep is not noticed until the sleep ends. A scenario
// about what an interrupted handler logs needs the interrupt to actually
// arrive, which means a loop the engine keeps stepping through.

// The image ships log_errors=Off, so PHP's own error log — which reaches the
// server through the SAPI's log hook, a second path with a second level of its
// own — is never written to. Turned on here so a scenario counting ERROR lines
// covers both places a fatal can be reported from and not just one.
ini_set('log_errors', '1');

$ms = isset($_GET['ms']) ? (int) $_GET['ms'] : 100;
$ms = max(0, min($ms, 30000));
$until = microtime(true) + $ms / 1000;
while (microtime(true) < $until) {
}

header('Content-Type: text/plain');
echo "spun {$ms}ms\n";
