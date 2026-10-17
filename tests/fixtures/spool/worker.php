<?php
// Two requests multiplexed on one worker thread, each writing past the
// in-memory threshold on both sides of a park: each body must be its own.
oxphp_worker(function () {
    $id = (int) ($_GET['id'] ?? 65);
    header('Content-Type: application/octet-stream');
    error_log("spool-worker: start $id");
    echo str_repeat(chr($id), 2 << 20);
    oxphp_sleep(0.5);
    echo str_repeat(chr($id), 2 << 20);
    error_log("spool-worker: end $id");
});
