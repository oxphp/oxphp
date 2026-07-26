<?php
// Holds a worker for ?ms= milliseconds (default 100, capped at 30s) so a test
// can control exactly how long the pool stays busy.
$ms = isset($_GET['ms']) ? (int) $_GET['ms'] : 100;
$ms = max(0, min($ms, 30000));
usleep($ms * 1000);

header('Content-Type: text/plain');
echo "paused {$ms}ms\n";
