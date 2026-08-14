<?php

declare(strict_types=1);

// Served in its own request fiber and parks there for longer than the outer
// request that opened it, so the outer resumes while THIS request is still in
// flight. That is the second failure mode of a start time held in one slot per
// worker thread: the outer used to come back reading the time below rather than
// 0.0, because nothing had ended and zeroed the slot yet.
//
// Reports its own start time and the moment it finished. The outer needs both:
// the start time to assert it did not inherit it, and the end time to prove
// this request was still running when the outer woke up — without which the
// whole case proves nothing.

$request = oxphp_http_request();
$start = $request->startTime(true);

sleep(3);                                   // hooked: parks this request fiber

echo json_encode([
    'marker' => 'SLOW-DONE',
    'start'  => $start,
    'end'    => microtime(true),
]);
