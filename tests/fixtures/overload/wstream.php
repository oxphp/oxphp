<?php
// Worker-mode twin of stream.php: same stream, reached through the worker
// entry point instead of by direct .php dispatch.
oxphp_worker(function () {
    $n = isset($_GET['n']) ? max(0, min((int) $_GET['n'], 1000)) : 200;
    usleep(max(0, min(isset($_GET['ms']) ? (int) $_GET['ms'] : 0, 30000)) * 1000);
    if ($n > 0) {
        register_shutdown_function(function () {
            file_put_contents('/tmp/streams', (connection_aborted() ? 'aborted' : 'complete') . "\n", FILE_APPEND);
        });
    }
    header('Content-Type: text/event-stream');
    header('Cache-Control: no-cache');
    for ($i = 0; $i < $n && !connection_aborted(); $i++) {
        echo "data: {$i}\n\n";
        oxphp_stream_flush();
        usleep(50000);
    }
});
