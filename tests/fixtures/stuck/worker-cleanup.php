<?php
// Worker-mode fixture for tests/stuck_classification.sh: a request parked in a
// cooperative sleep, which the drain deadline's sweep resumes and unwinds.
oxphp_worker(function () {
    set_time_limit(0);
    register_shutdown_function(function () {
        // Runs after the unwind, on a request still past the threshold: the
        // supervisor keeps interrupting it, and that must not end it a second
        // time.
        $until = microtime(true) + 1.3;
        while (microtime(true) < $until) {
        }
        error_log('stuck-cleanup: shutdown function finished');
    });
    oxphp_sleep(3600);
});
