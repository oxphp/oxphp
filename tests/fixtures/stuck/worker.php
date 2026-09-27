<?php
// Worker-mode fixture for tests/stuck_classification.sh: the call-free loop,
// run from a request fiber.
oxphp_worker(function () {
    set_time_limit(0);
    $i = 0;
    for (;;) {
        $i++;
    }
});
