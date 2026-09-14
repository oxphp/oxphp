<?php
// Worker-mode twin of pause.php: same handler, reached through the worker
// entry point instead of by direct .php dispatch.
oxphp_worker(function () {
    $ms = isset($_GET['ms']) ? (int) $_GET['ms'] : 100;
    $ms = max(0, min($ms, 30000));
    usleep($ms * 1000);
    header('Content-Type: text/plain');
    echo "paused {$ms}ms\n";
});
