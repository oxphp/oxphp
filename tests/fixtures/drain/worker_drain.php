<?php
// Worker-mode fixture for tests/graceful_drain.sh. Each route parks the
// request in a different long-lived shape so the drain test can assert that
// SIGTERM cancels every one of them — and spares the short-lived one.
oxphp_worker(function () {
    $path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH);
    switch ($path) {
        case '/sse': // streaming + cooperative sleep (fiber SLEEP suspend)
            header('Content-Type: text/event-stream');
            for ($i = 0; ; $i++) {
                echo "data: tick $i\n\n";
                oxphp_stream_flush();
                oxphp_sleep(0.5);
            }
            // unreachable

        case '/await': // fiber AWAIT suspend on a promise that never settles in time
            header('Content-Type: text/event-stream');
            echo "data: awaiting\n\n";
            oxphp_stream_flush();
            oxphp_async_await(oxphp_async(function (): int {
                oxphp_sleep(3600.0);
                return 1;
            }));
            echo "data: unreachable\n\n";
            return;

        case '/empty': // streaming whose later flushes carry no bytes
            header('Content-Type: text/event-stream');
            echo "data: first\n\n";
            oxphp_stream_flush();
            for (;;) {
                oxphp_sleep(0.5);
                oxphp_stream_flush(); // nothing buffered — drain must still cancel
            }
            // unreachable

        case '/catch': // userland tries to swallow the drain bail
            header('Content-Type: text/event-stream');
            for ($i = 0; ; $i++) {
                try {
                    echo "data: tick $i\n\n";
                    oxphp_stream_flush();
                    oxphp_sleep(0.5);
                } catch (\Throwable $e) {
                    // must never swallow the shutdown bail
                }
            }
            // unreachable

        case '/bg':   // stream that finishes its response, then does background
                      // work. The closing flush inside oxphp_finish_request()
                      // must not self-cancel it while draining, and after
                      // finish the suspended fiber counts as ordinary — the
                      // soft sweep must spare it so 'bg-done' gets logged.
            header('Content-Type: text/event-stream');
            echo "data: bg-start\n\n";
            oxphp_stream_flush();
            usleep((int)($_GET['ms'] ?? 2500) * 1000); // native: spans SIGTERM
            oxphp_finish_request();
            oxphp_sleep(2.0); // post-response work, cooperatively suspended
            error_log('bg-done');
            return;

        case '/bgplain': // ORDINARY (non-streaming) response finished early,
                         // then long background work. Unlike /bg this takes the
                         // early-send oneshot path: the whole response goes out
                         // at once, the connection winds down, and the live
                         // connection count reaches zero while the worker is
                         // still executing post-response work. The drain must
                         // still apply its deadline to that work.
            echo 'bgplain-start';
            oxphp_finish_request();
            oxphp_sleep((float)($_GET['post'] ?? 30));
            error_log('bgplain-done');
            return;

        case '/tight': // streaming request that never suspends: it flushes in a
                       // loop with a native usleep, so the scheduler's drain
                       // sweep never sees it. Only the stream-flush path's
                       // self-cancel can end it — and that unwind must count as
                       // an administrative drain kill, not a handler failure,
                       // or three of them in a row trip the worker's
                       // consecutive-error breaker.
            header('Content-Type: text/event-stream');
            for ($i = 0; ; $i++) {
                echo "data: tick $i\n\n";
                oxphp_stream_flush();
                usleep(20000); // native: does NOT suspend the fiber
            }
            // unreachable

        case '/short': // ordinary request: native blocking sleep, no flush
            usleep((int)($_GET['ms'] ?? 3000) * 1000);
            echo 'short-done';
            return;

        case '/pause': // ordinary request suspended cooperatively, NO streaming —
                       // the soft drain phase must leave it alone (only the
                       // deadline sweep may kill it)
            oxphp_sleep((float)($_GET['s'] ?? 2));
            echo 'pause-done';
            return;

        case '/spin': // CPU-bound loop: only a vm_interrupt kick can reach it
            for (;;) {
                // burn until cancelled
            }
            // unreachable

        default:
            echo 'ok';
            return;
    }
});
