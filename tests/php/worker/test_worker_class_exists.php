<?php
foreach (['OxPHP\\Server\\Worker', 'OxPHP\\Server\\Exception\\InvalidServeContextException'] as $fqn) {
    if (!class_exists($fqn)) {
        http_response_code(500);
        echo "FAIL: class $fqn does not exist\n";
        exit;
    }
}

$wRefl = new ReflectionClass('OxPHP\\Server\\Worker');
if (!$wRefl->isFinal()) {
    http_response_code(500);
    echo "FAIL: Worker is not final\n";
    exit;
}

$exRefl = new ReflectionClass('OxPHP\\Server\\Exception\\InvalidServeContextException');
if (!$exRefl->isSubclassOf('RuntimeException')) {
    http_response_code(500);
    echo "FAIL: InvalidServeContextException does not extend RuntimeException\n";
    exit;
}

$expected = ['current', 'isWorkerMode', 'id', 'startTime',
             'requestCount', 'memoryUsage', 'rss',
             'maxMemoryBytes', 'serve'];
foreach ($expected as $m) {
    if (!$wRefl->hasMethod($m)) {
        http_response_code(500);
        echo "FAIL: Worker is missing method $m\n";
        exit;
    }
}

echo "OK\n";
