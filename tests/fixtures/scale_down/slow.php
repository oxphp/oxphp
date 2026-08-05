<?php
// Traditional-mode fixture for tests/worker_scale_down.sh: occupies the worker
// that picks it up for longer than the scenario's idle threshold, without
// letting it come back for another request in the meantime. Names the worker
// so the scenario can tell which ones were occupied.
header('Content-Type: text/plain');
$seconds = isset($_GET['s']) ? min(30, max(1, (int) $_GET['s'])) : 8;
sleep($seconds);
echo 'slow worker ', oxphp_worker_id(), "\n";
