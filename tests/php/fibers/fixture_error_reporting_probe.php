<?php

declare(strict_types=1);

// Inner self-request for fibers/test_error_reporting_is_not_inherited.
//
// ?phase=set lowers the reporting level to a value nothing else uses and
// finishes. ?phase=read reports the level it starts with. Both report which
// fiber served them, because the question is what a fiber carries from one
// request into the next it serves.
//
// Must not suspend: both phases have to run and finish inside the window the
// outer request is parked for.

// A level no php.ini or boot script sets: everything but E_USER_NOTICE. A
// variable, not a constant: in worker mode this file runs again on the same
// thread, and a second top-level const would redeclare the first.
$sentinel = E_ALL & ~E_USER_NOTICE;

header('Content-Type: application/json');

$fiber = \Fiber::getCurrent();
$report = [
    'fiber' => $fiber === null ? null : spl_object_id($fiber),
];

if (($_GET['phase'] ?? '') === 'set') {
    error_reporting($sentinel);
    $report['level'] = error_reporting();
} else {
    $report['level'] = error_reporting();
    $report['ini'] = ini_get('error_reporting');
}

$report['sentinel'] = $sentinel;

echo json_encode($report);
