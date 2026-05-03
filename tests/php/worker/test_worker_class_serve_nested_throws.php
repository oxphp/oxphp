<?php
/* In worker mode, calling serve() inside an already-running serve()
 * must throw InvalidServeContextException with "nested" in the message.
 * In traditional mode, no-op. */

$w = OxPHP\Server\Worker::current();
if (!$w->isWorkerMode()) {
    echo "OK (traditional mode — test only meaningful in worker)\n";
    exit;
}

try {
    $w->serve(function () { /* never invoked */ });
} catch (OxPHP\Server\InvalidServeContextException $e) {
    if (str_contains($e->getMessage(), 'nested')) {
        echo "OK threw on nested: " . $e->getMessage() . "\n";
        exit;
    }
    http_response_code(500);
    echo "FAIL: wrong message: " . $e->getMessage() . "\n";
    exit;
}

http_response_code(500);
echo "FAIL: no exception thrown\n";
