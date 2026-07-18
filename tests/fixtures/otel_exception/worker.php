<?php
// Worker-mode entry. The fiber harness catches an uncaught handler exception (it
// never reaches zend_exception_error), so OxPHP captures it at the catch site
// and the root SERVER span still carries an exception event.
//
//  /boom    — throws directly in the handler body from a named frame so the
//             stacktrace (workerBoom) and file/line are assertable. The response
//             MUST be a 500: the fiber path cannot drive status off the (never
//             set) ctx.handler_failed, so the capture itself must force it — else
//             the root-span gate (>=500) drops the event.
//  /stream-boom — commits a 5xx, streams a chunk, THEN throws. The status ships,
//             but the request has already completed once the headers went out, so
//             the late fatal is logged only and does NOT reach the span (a
//             documented streaming boundary; the test asserts its absence).
//  /shadow  — throws in the handler AND registers a shutdown function that
//             raises its own E_USER_ERROR. The shutdown error is recorded first
//             (during php_call_shutdown_functions, before the fiber capture is
//             pulled in), so the root span must still report the handler's
//             exception — the killer must lead the error stream, not the later
//             shutdown-time error.
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

    if ($uri === '/stream-boom') {
        // Worker STREAMING failure: commit a 5xx, flush headers + a body chunk,
        // THEN throw. The status is already on the wire and the request has
        // completed, so the late fatal is logged only and does NOT land on the
        // span — the documented streaming boundary, same as the traditional path.
        http_response_code(500);
        header('Content-Type: text/plain');
        echo 'stream-boom:partial';
        oxphp_stream_flush();
        throw new RuntimeException('worker stream fatal after headers');
    }

    if ($uri === '/shadow') {
        // The handler's exception is the real killer. A shutdown function then
        // raises its own E_USER_ERROR, which oxphp_error_cb records into
        // REQUEST_ERRORS *before* the fiber capture is pulled in at send time.
        // The root span must report the handler killer (inserted at the front of
        // the error stream), not the shadowing shutdown error.
        register_shutdown_function(function () {
            trigger_error('shadow shutdown blew up', E_USER_ERROR);
        });
        throw new RuntimeException('shadow handler killer');
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
        // A class distinct from every other scenario's (MANUAL uses
        // LogicException) so the shared-collector grep for this capture cannot
        // pass on another span's event.
        throw new UnderflowException('scenario-b: parked capture survived');
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
