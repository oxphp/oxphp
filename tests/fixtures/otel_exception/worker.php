<?php
// Worker-mode entry. The fiber harness catches an uncaught handler exception (it
// never reaches zend_exception_error), so OxPHP captures it at the catch site
// and the root SERVER span still carries an exception event.
//
//  /boom    — throws from a named frame so the stacktrace (workerBoom) and the
//             file/line extension are assertable, not just the message.
//  /a-fail  — throws, then registers a shutdown function that SUSPENDS
//             (oxphp_sleep). The capture is parked with this fiber while another
//             request runs on the same worker thread; per-fiber save/restore
//             must keep it from being wiped by that request's reset.
//  /b-ok    — a fast 200, driven concurrently while /a-fail is parked.
function workerBoom(): void {
    throw new RuntimeException('worker path: handler exploded');
}

oxphp_worker(function () {
    $uri = $_SERVER['REQUEST_URI'] ?? '/';

    if ($uri === '/boom') {
        workerBoom();
    }

    if ($uri === '/a-fail') {
        register_shutdown_function(function () {
            oxphp_sleep(800);
        });
        throw new LogicException('scenario-b: parked capture survived');
    }

    if ($uri === '/b-ok') {
        header('Content-Type: text/plain');
        echo 'b-ok';
        return;
    }

    header('Content-Type: text/plain');
    echo 'ok';
});
