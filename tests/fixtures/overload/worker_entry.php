<?php
// Worker-mode twin of pause.php and spin.php: holds the worker for ?ms=
// milliseconds, sleeping by default and burning opcodes with ?spin=1.
//
// Worker mode reaches the queue through a different pickup path than the
// traditional loop does, and unwinds an interrupted handler through the fiber
// scheduler rather than straight out of the worker thread, so the checks about
// what a departed client costs have to be run against both. Everything else
// about them is the same on purpose — same query parameters, same cap, same
// body.
oxphp_worker(function () {
    $ms = isset($_GET['ms']) ? (int) $_GET['ms'] : 100;
    $ms = max(0, min($ms, 30000));

    if (isset($_GET['spin'])) {
        // See spin.php: covers the SAPI log hook as well as the error callback.
        ini_set('log_errors', '1');
        $until = microtime(true) + $ms / 1000;
        while (microtime(true) < $until) {
        }
        header('Content-Type: text/plain');
        echo "spun {$ms}ms\n";
        return;
    }

    usleep($ms * 1000);
    header('Content-Type: text/plain');
    echo "paused {$ms}ms\n";
});
