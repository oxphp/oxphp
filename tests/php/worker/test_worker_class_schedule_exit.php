<?php
$w = OxPHP\Server\Worker::current();

if (!$w->isWorkerMode()) {
    /* Traditional mode: scheduleExit() is a no-op, isExitScheduled() always false. */
    $w->scheduleExit();
    if ($w->isExitScheduled() !== false) {
        http_response_code(500);
        echo "FAIL: traditional isExitScheduled() should always be false\n";
        exit;
    }
    if ($w->getExitReason() !== null) {
        http_response_code(500);
        echo "FAIL: traditional getExitReason() should be null\n";
        exit;
    }
    echo "OK (traditional)\n";
    exit;
}

/* Worker mode: this request schedules exit; loop check stops the worker
 * after we return. We only verify state observable from inside the request. */
if ($w->isExitScheduled() !== false) {
    http_response_code(500);
    echo "FAIL: isExitScheduled() must be false before scheduleExit()\n";
    exit;
}
if ($w->getExitReason() !== null) {
    http_response_code(500);
    echo "FAIL: getExitReason() must be null before scheduleExit()\n";
    exit;
}

$w->scheduleExit();

if ($w->isExitScheduled() !== true) {
    http_response_code(500);
    echo "FAIL: isExitScheduled() must be true after scheduleExit()\n";
    exit;
}
if ($w->getExitReason() !== 'scheduled') {
    http_response_code(500);
    echo "FAIL: getExitReason() = " . var_export($w->getExitReason(), true) . " (want 'scheduled')\n";
    exit;
}

/* Idempotency: second call must not change anything. */
$w->scheduleExit();
if ($w->isExitScheduled() !== true || $w->getExitReason() !== 'scheduled') {
    http_response_code(500);
    echo "FAIL: scheduleExit() not idempotent\n";
    exit;
}

echo "OK\n";
