<?php
// Worker-mode entry. The fiber harness catches an uncaught handler exception (it
// never reaches zend_exception_error), so OxPHP captures it at the catch site
// and the root SERVER span still carries an exception event.
//
//  /boom    — throws from a named frame so the stacktrace (workerBoom) and the
//             file/line extension are assertable, not just the message.
//  /a-fail  — throws, then registers a shutdown function that writes a marker
//             and SUSPENDS (oxphp_sleep). The capture is parked with this fiber
//             while /b-ok runs on the same worker thread; per-fiber save/restore
//             must keep it from being wiped by that request's reset.
//  /b-ok    — spins until it observes /a-fail's marker (deterministic overlap:
//             it can only succeed while /a-fail is parked), then a fast 200.
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
            // Signal that this request has entered its parked phase, then SUSPEND
            // (oxphp_sleep) so /b-ok runs on this same single-thread worker while
            // the unhandled-exception capture is parked with this fiber. Bounded
            // so the request still finishes — and its span flushes — inside the
            // test window (oxphp_sleep takes SECONDS).
            @file_put_contents('/tmp/a_parked', '1');
            oxphp_sleep(2.0);
        });
        throw new LogicException('scenario-b: parked capture survived');
    }

    if ($uri === '/b-ok') {
        // Deterministic overlap: only report success once /a-fail is observably
        // parked, so this can never pass without a genuine overlap on the single
        // worker thread. oxphp_usleep suspends the fiber cooperatively, letting
        // /a-fail run to its parked phase even if /b-ok was dispatched first.
        $overlapped = false;
        for ($i = 0; $i < 500; $i++) { // up to ~5s
            clearstatcache(true, '/tmp/a_parked');
            if (is_file('/tmp/a_parked')) {
                $overlapped = true;
                break;
            }
            oxphp_usleep(10000); // 10ms
        }
        header('Content-Type: text/plain');
        echo $overlapped ? 'b-ok:overlapped' : 'b-ok:no-overlap';
        return;
    }

    header('Content-Type: text/plain');
    echo 'ok';
});
