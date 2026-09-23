<?php

declare(strict_types=1);

// Inner self-request for fibers/test_ini_is_the_requests_own.
//
// Reports the ini values it finds at its own start — which must be the ones the
// worker booted with, not the ones the parked request set — and then changes
// two directives of its own: one the parked request also changed, and one it
// did not touch. Both must be gone by the time the parked request resumes.
//
// And a third, open_basedir, which does not travel: its handler does more than
// store the value, so it stays on the worker while any request is in flight and
// is put back when the last one ends — from a script it can only be tightened,
// so that has to happen at the stage PHP itself restores directives at.
// Tightened last, once this request has nothing left to open.
//
// Must not suspend: this request has to run and finish inside the window the
// outer one is parked for.

header('Content-Type: application/json');

$seen = [
    'precision'         => ini_get('precision'),
    'user_agent'        => ini_get('user_agent'),
    'ignore_user_abort' => ini_get('ignore_user_abort'),
];

// Values neither the boot script nor the parked request uses.
ini_set('precision', '11');
ini_set('user_agent', 'oxphp-inner-ini-probe');
ini_set('open_basedir', '/var/www/html:/tmp');

echo json_encode([
    'seen' => $seen,
    'set'  => [
        'precision'  => ini_get('precision'),
        'user_agent' => ini_get('user_agent'),
        'open_basedir' => ini_get('open_basedir'),
    ],
]);
