<?php
// Worker-mode fixture for tests/worker_scale_down.sh. The scenario is about the
// pool's scale-down path, not about the handler, so the handler does the least
// a handler can do — but it names the worker thread that answered rather than
// the process, because in worker mode every worker is a thread of the same
// process and a pid would identify nothing.
oxphp_worker(function () {
    header('Content-Type: text/plain');
    echo 'worker ', oxphp_worker_id(), "\n";
});
