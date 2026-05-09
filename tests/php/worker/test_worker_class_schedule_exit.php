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
    if ($w->exitReason() !== null) {
        http_response_code(500);
        echo "FAIL: traditional exitReason() should be null\n";
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
if ($w->exitReason() !== null) {
    http_response_code(500);
    echo "FAIL: exitReason() must be null before scheduleExit()\n";
    exit;
}

$w->scheduleExit();

if ($w->isExitScheduled() !== true) {
    http_response_code(500);
    echo "FAIL: isExitScheduled() must be true after scheduleExit()\n";
    exit;
}
if ($w->exitReason() !== 'scheduled') {
    http_response_code(500);
    echo "FAIL: exitReason() = " . var_export($w->exitReason(), true) . " (want 'scheduled')\n";
    exit;
}

/* Idempotency: second call must not change anything. */
$w->scheduleExit();
if ($w->isExitScheduled() !== true || $w->exitReason() !== 'scheduled') {
    http_response_code(500);
    echo "FAIL: scheduleExit() not idempotent\n";
    exit;
}

echo "OK\n";
