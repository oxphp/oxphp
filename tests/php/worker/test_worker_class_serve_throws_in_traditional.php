<?php
/* In traditional mode, Worker::serve() must throw InvalidServeContextException.
 * If running under worker mode, this test is a no-op (echo OK). */

$w = OxPHP\Server\Worker::current();
if ($w->isWorkerMode()) {
    echo "OK (worker mode — test only meaningful in traditional)\n";
    exit;
}

try {
    $w->serve(function () { /* never invoked */ });
} catch (OxPHP\Server\Exception\InvalidServeContextException $e) {
    if (str_contains($e->getMessage(), 'worker mode')) {
        echo "OK threw as expected: " . $e->getMessage() . "\n";
        exit;
    }
    http_response_code(500);
    echo "FAIL: wrong message: " . $e->getMessage() . "\n";
    exit;
}

http_response_code(500);
echo "FAIL: no exception thrown\n";
